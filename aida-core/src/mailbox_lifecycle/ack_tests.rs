//! B2c fixtures: the post-flush ACK plan and the common-Git receipt anchor.
//! trace:STORY-1843 | ai:claude

use super::*;
use crate::mailbox::{write_local_message, Recipient};
use crate::mailbox_lifecycle::tests::{edit_record, message, repo};
use crate::mailbox_logical::{
    logical_snapshot, AckOrigin, MailboxContext, Query, ReadLimits, ReceiptInspector, ReceiptLookup,
};

fn bob() -> ResolvedAudience {
    ResolvedAudience::from_identity_policy(vec!["bob".into()], vec![], vec![]).unwrap()
}

/// A reader with two inboxes: `bob` and the shared `advisor` role.
fn bob_and_advisor() -> ResolvedAudience {
    ResolvedAudience::from_identity_policy(vec!["bob".into()], vec![], vec!["advisor".into()])
        .unwrap()
}

fn snapshot(root: &Path) -> (MailboxContext, LogicalSnapshot) {
    snapshot_for(root, &bob())
}

fn snapshot_for(root: &Path, audience: &ResolvedAudience) -> (MailboxContext, LogicalSnapshot) {
    let ctx = MailboxContext::resolve_or_local(root, None).unwrap();
    let snap = logical_snapshot(&ctx, audience, &Query::Inbox, &ReadLimits::explicit_read());
    (ctx, snap)
}

/// Plan the ACK of every record the snapshot holds, as an explicit read
/// that displays the whole inbox would.
fn plan(root: &Path) -> Result<Option<AckPlan>, AckError> {
    plan_for(root, &bob())
}

fn plan_for(root: &Path, audience: &ResolvedAudience) -> Result<Option<AckPlan>, AckError> {
    let (ctx, snap) = snapshot_for(root, audience);
    let all: Vec<&SnapshotRecord> = snap.records.iter().collect();
    AckPlan::prepare(&snap, ctx.repository().map(|r| r.path()), audience, &all)
}

fn read_of(root: &Path, id: &str) -> ReadEvidence {
    read_for(root, &bob(), id)
}

fn read_for(root: &Path, audience: &ResolvedAudience, id: &str) -> ReadEvidence {
    let (_, snap) = snapshot_for(root, audience);
    snap.records
        .iter()
        .find(|r| r.message.id == id)
        .map(|r| r.read.clone())
        .expect("record is visible")
}

fn ledger_path(root: &Path) -> PathBuf {
    ledger_path_for(root, "bob")
}

fn ledger_path_for(root: &Path, key: &str) -> PathBuf {
    common_of(root)
        .join("aida/mail-lifecycle/receipts")
        .join(receipt_ledger_name(key))
}

fn common_of(root: &Path) -> PathBuf {
    let ctx = MailboxContext::resolve(root, None).unwrap();
    ctx.repository().unwrap().path().to_path_buf()
}

fn ledger_holds(root: &Path, key: &str, id: &str) -> bool {
    read_receipt_ledger(&common_of(root), key)
        .unwrap()
        .is_some_and(|l| l.receipts.contains_key(id))
}

fn anchor_of(root: &Path) -> PathBuf {
    let ctx = MailboxContext::resolve(root, None).unwrap();
    ctx.repository().unwrap().path().join("aida/mail-lifecycle")
}

#[test]
fn story1843_b2c_commit_anchors_receipts_that_outlive_the_source() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    assert_eq!(read_of(root, "m1"), ReadEvidence::Unread);

    let plan = plan(root).unwrap().expect("one unread record");
    assert_eq!(plan.len(), 1);
    // Preparing records nothing.
    assert!(!ledger_path(root).exists());
    assert_eq!(read_of(root, "m1"), ReadEvidence::Unread);

    let committed = plan.commit(&bob()).unwrap();
    assert!(committed.anchored);
    assert_eq!(committed.messages(), 1);
    assert!(matches!(
        read_of(root, "m1"),
        ReadEvidence::Acknowledged {
            origin: AckOrigin::Anchor { legacy: false },
            ..
        }
    ));
    // The fleet-wide inspector reads the same anchor.
    let ctx = MailboxContext::resolve(root, None).unwrap();
    let inspector = ReceiptInspector::new(&ctx, &ReadLimits::explicit_read());
    assert!(matches!(
        inspector.lookup("bob", "m1"),
        ReceiptLookup::Read { .. }
    ));
    // No local receipt was written by the core commit, and removing every
    // local receipt (or the whole source) cannot resurrect the message.
    std::fs::remove_dir_all(root.join(".aida/mailbox/.read-receipts")).ok();
    assert!(matches!(
        read_of(root, "m1"),
        ReadEvidence::Acknowledged {
            origin: AckOrigin::Anchor { legacy: false },
            ..
        }
    ));
    // Nothing left to plan.
    assert!(plan_none(root));
}

fn plan_none(root: &Path) -> bool {
    matches!(plan(root), Ok(None))
}

#[test]
fn story1843_b2c_commit_is_idempotent_and_keeps_the_first_ack_time() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    let plan = plan(root).unwrap().unwrap();
    plan.clone().commit(&bob()).unwrap();
    let first = std::fs::read(ledger_path(root)).unwrap();
    std::thread::sleep(Duration::from_millis(5));
    plan.commit(&bob()).unwrap();
    assert_eq!(std::fs::read(ledger_path(root)).unwrap(), first);
}

#[test]
fn story1843_b2c_unrelated_arrival_is_never_added_to_a_plan() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    let plan = plan(root).unwrap().unwrap();
    write_local_message(root, &message("m2")).unwrap();
    plan.commit(&bob()).unwrap();
    assert!(matches!(
        read_of(root, "m1"),
        ReadEvidence::Acknowledged { .. }
    ));
    assert_eq!(read_of(root, "m2"), ReadEvidence::Unread);
}

#[test]
fn story1843_b2c_changed_replica_refuses_and_writes_nothing() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    let plan = plan(root).unwrap().unwrap();
    // A tombstone (or any edit) after the plan changes the bytes.
    let mut retracted = message("m1");
    retracted.retracted = true;
    write_local_message(root, &retracted).unwrap();
    assert!(matches!(plan.commit(&bob()), Err(AckError::Changed)));
    assert!(!ledger_path(root).exists());
}

#[test]
fn story1843_b2c_retiring_source_refuses_and_writes_nothing() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    let plan = plan(root).unwrap().unwrap();
    edit_record(root, |r| {
        r.state = SourceState::Fenced {
            op_id: "op".into(),
            reason: "test".into(),
        }
    });
    assert!(matches!(plan.commit(&bob()), Err(AckError::Retiring)));
    assert!(!ledger_path(root).exists());
    // A fenced source is not planned at all.
    assert!(matches!(self::plan(root), Err(AckError::Retiring)));
}

#[test]
fn story1843_b2c_authority_is_rechecked_before_any_lock() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    let plan = plan(root).unwrap().unwrap();
    let other =
        ResolvedAudience::from_identity_policy(vec!["carol".into()], vec![], vec![]).unwrap();
    let (result, classes) = super::super::observe_lock_classes(|| plan.commit(&other));
    assert!(matches!(result, Err(AckError::Authority)));
    assert!(classes.is_empty(), "{classes:?}");
    assert!(!ledger_path(root).exists());
}

#[test]
fn story1843_b2c_lock_order_is_lifecycle_then_receipt() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    let plan = plan(root).unwrap().unwrap();
    let (result, classes) = super::super::observe_lock_classes(|| plan.commit(&bob()));
    result.unwrap();
    assert_eq!(classes, vec!["lifecycle", "receipt"]);
}

#[test]
fn story1843_b2c_busy_receipt_lock_refuses_bounded() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    let plan = plan(root).unwrap().unwrap();
    let anchor = anchor_of(root);
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(anchor.join(RECEIPT_LOCK_FILE))
        .unwrap();
    fs2::FileExt::lock_exclusive(&holder).unwrap();
    let start = Instant::now();
    let result = plan.commit(&bob());
    assert!(
        matches!(
            result,
            Err(AckError::Lifecycle(LifecycleError::LockTimeout { .. }))
        ),
        "{result:?}"
    );
    assert!(start.elapsed() < RECEIPT_LOCK_TIMEOUT + Duration::from_secs(2));
    assert!(!ledger_path(root).exists());
    assert_eq!(read_of(root, "m1"), ReadEvidence::Unread);
    // Released, a fresh plan of the same read commits.
    fs2::FileExt::unlock(&holder).unwrap();
    self::plan(root).unwrap().unwrap().commit(&bob()).unwrap();
    assert!(matches!(
        read_of(root, "m1"),
        ReadEvidence::Acknowledged { .. }
    ));
}

#[test]
fn story1843_b2c_full_ledger_refuses_without_pruning() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    let plan = plan(root).unwrap().unwrap();
    let full = ReceiptLedger {
        schema: RECEIPT_LEDGER_SCHEMA,
        key: "bob".into(),
        receipts: (0..RECEIPT_LEDGER_MAX_ENTRIES)
            .map(|i| (format!("old-{i}"), 1))
            .collect(),
        legacy: Default::default(),
    };
    let path = ledger_path(root);
    ensure_dir(path.parent().unwrap(), true).unwrap();
    write_durable(&path, &serde_json::to_vec(&full).unwrap()).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(plan.commit(&bob()), Err(AckError::LedgerFull)));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(read_of(root, "m1"), ReadEvidence::Unread);
}

#[test]
fn story1843_b2c_explicit_ack_resolves_a_legacy_watermark() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    let marks = root.join(".aida/mailbox/.read");
    std::fs::create_dir_all(&marks).unwrap();
    std::fs::write(marks.join("bob.txt"), "100").unwrap();
    assert!(matches!(
        read_of(root, "m1"),
        ReadEvidence::LegacyWatermark { .. }
    ));
    plan(root).unwrap().unwrap().commit(&bob()).unwrap();
    // Acknowledged, but with no read time claimed (TASK-1271).
    assert!(matches!(
        read_of(root, "m1"),
        ReadEvidence::Acknowledged {
            origin: AckOrigin::Anchor { legacy: true },
            ..
        }
    ));
    let ctx = MailboxContext::resolve(root, None).unwrap();
    let inspector = ReceiptInspector::new(&ctx, &ReadLimits::explicit_read());
    assert_eq!(inspector.lookup("bob", "m1"), ReceiptLookup::ReadUntimed);
}

/// A view with a refused source (here a store of another repository) still
/// shows the local record, but plans no ACK for it.
#[test]
fn story1843_b2c_partial_view_plans_nothing() {
    let dir = repo();
    let root = dir.path();
    let foreign = repo();
    write_local_message(root, &message("m1")).unwrap();
    let ctx = MailboxContext::resolve(root, Some(foreign.path())).unwrap();
    let snap = logical_snapshot(&ctx, &bob(), &Query::Inbox, &ReadLimits::explicit_read());
    assert!(!snap.complete());
    let all: Vec<&SnapshotRecord> = snap.records.iter().collect();
    assert_eq!(all.len(), 1);
    let planned = AckPlan::prepare(&snap, ctx.repository().map(|r| r.path()), &bob(), &all);
    assert!(matches!(planned, Err(AckError::Partial)));
}

#[test]
fn story1843_b2c_corrupt_ledger_makes_the_view_partial() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    plan(root).unwrap().unwrap().commit(&bob()).unwrap();
    std::fs::write(ledger_path(root), "{garbage").unwrap();
    let (_, snap) = snapshot(root);
    assert!(snap.anchor_error.is_some());
    assert!(!snap.complete());
    assert_eq!(snap.caught_up(), None);
    assert!(matches!(plan(root), Err(AckError::Partial)));
}

#[test]
// trace:TASK-2903 | ai:codex
fn story1843_b2c_local_only_project_commits_without_an_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    // Local-only resolution deliberately refuses even a malformed .git in
    // an ancestor. A shared temp root can have one; do not weaken that check
    // or modify the host's Git metadata just to construct this fixture.
    if let Some(ancestor) = root
        .ancestors()
        .find(|path| std::fs::symlink_metadata(path.join(".git")).is_ok())
    {
        eprintln!(
            "SKIP story1843_b2c_local_only_project_commits_without_an_anchor: \
             unsupported fixture environment: {} has a .git entry; \
             rerun with TMPDIR outside Git ancestry and --nocapture",
            ancestor.display()
        );
        return;
    }
    assert!(MailboxContext::resolve_or_local(&root, None)
        .unwrap()
        .repository()
        .is_none());
    write_local_message(&root, &message("m1")).unwrap();
    let committed = plan(&root).unwrap().unwrap().commit(&bob()).unwrap();
    assert!(!committed.anchored);
    assert_eq!(
        committed.by_key.get("bob").map(Vec::as_slice),
        Some(&["m1".to_string()][..])
    );
}

#[test]
// trace:TASK-2903 | ai:codex
fn local_only_ack_fixture_with_a_malformed_git_ancestor_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let root = dir.path().join("local-project");
    std::fs::create_dir(&root).unwrap();
    assert_eq!(
        MailboxContext::resolve_or_local(&root, None).unwrap_err(),
        crate::mailbox_logical::SourceErrorCode::NotAGitCheckout
    );
    assert!(!root.join(".aida").exists());
}

/// Advisor ruling on signoff 60 #5: each key's ledger is its own rename, so
/// a write that fails after the first key's rename leaves a partial ACK. The
/// commit says so; the message is still unread for the read (only one of
/// its keys holds it); the retry plans exactly the missing key and
/// converges, keeping the first key's ACK time.
#[test]
fn story1843_b2c_partial_publish_converges_on_retry() {
    let dir = repo();
    let root = dir.path();
    let mut m1 = message("m1");
    m1.to = Recipient::Broadcast;
    write_local_message(root, &m1).unwrap();
    let both = bob_and_advisor();
    let plan = plan_for(root, &both).unwrap().unwrap();
    assert_eq!(
        plan.by_key().keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["advisor", "bob"]
    );

    // The second ledger write fails after `advisor`'s rename.
    FAIL_PUBLISH_AFTER.with(|f| f.set(Some(1)));
    let result = plan.commit(&both);
    FAIL_PUBLISH_AFTER.with(|f| f.set(None));
    let err = result.unwrap_err();
    assert!(matches!(err, AckError::Incomplete(_)), "{err:?}");
    assert!(
        err.to_string().contains("only some of its inboxes"),
        "{err}"
    );
    assert!(ledger_holds(root, "advisor", "m1"));
    assert!(!ledger_holds(root, "bob", "m1"));
    // One key alone is not an acknowledgement of the read.
    assert_eq!(read_for(root, &both, "m1"), ReadEvidence::Unread);
    let advisor_before = std::fs::read(ledger_path_for(root, "advisor")).unwrap();

    // The retry plans exactly the key still missing the receipt.
    let retry = plan_for(root, &both).unwrap().unwrap();
    assert_eq!(
        retry.by_key(),
        BTreeMap::from([("bob".to_string(), vec!["m1".to_string()])])
    );
    std::thread::sleep(Duration::from_millis(5));
    retry.commit(&both).unwrap();
    assert!(ledger_holds(root, "advisor", "m1"));
    assert!(ledger_holds(root, "bob", "m1"));
    assert_eq!(
        std::fs::read(ledger_path_for(root, "advisor")).unwrap(),
        advisor_before,
        "the converged key keeps its first ACK"
    );
    assert!(matches!(
        read_for(root, &both, "m1"),
        ReadEvidence::Acknowledged {
            origin: AckOrigin::Anchor { legacy: false },
            ..
        }
    ));
    assert!(matches!(plan_for(root, &both), Ok(None)));
}

/// Signoff 60 #8: the same message ID with different bytes in two replicas
/// (here main and a sibling worktree) is a conflict, and a view with a
/// conflict plans no ACK, even for the other, agreeing records it shows.
#[test]
fn story1843_b2c_conflicting_replicas_plan_nothing() {
    let dir = repo();
    let root = dir.path();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    let sib = root.join("sib");
    git(&["worktree", "add", "-q", "--detach", sib.to_str().unwrap()]);
    write_local_message(root, &message("m1")).unwrap();
    let mut diverged = message("m1");
    diverged.body = "a different body".into();
    write_local_message(&sib, &diverged).unwrap();
    write_local_message(root, &message("m2")).unwrap();

    let (_, snap) = snapshot(root);
    assert_eq!(snap.conflicts, vec!["m1"]);
    assert!(!snap.ack_eligible());
    assert!(matches!(plan(root), Err(AckError::Partial)));
    assert!(!ledger_path(root).exists());
    assert_eq!(read_of(root, "m2"), ReadEvidence::Unread);
}

/// Signoff 60 #6: the inspector's ledger reads draw from the same deadline
/// and byte budget as its inventory. Past either, a receipt that IS in the
/// anchor answers Unknown (partial), never a read and never "no receipt".
#[test]
fn story1843_b2c_receipt_inspector_honours_deadline_and_byte_budget() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    plan(root).unwrap().unwrap().commit(&bob()).unwrap();
    let ctx = MailboxContext::resolve(root, None).unwrap();
    assert!(matches!(
        ReceiptInspector::new(&ctx, &ReadLimits::explicit_read()).lookup("bob", "m1"),
        ReceiptLookup::Read { .. }
    ));

    // Past the deadline the ledger is not read.
    let limits = ReadLimits::with_deadline(Instant::now() + Duration::from_millis(300));
    let late = ReceiptInspector::new(&ctx, &limits);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(late.lookup("bob", "m1"), ReceiptLookup::Unknown);
    assert!(late.partial());

    // A ledger larger than what the budget has left is not read either.
    let mut ledger = read_receipt_ledger(&common_of(root), "bob")
        .unwrap()
        .unwrap();
    ledger
        .receipts
        .extend((0..2_000).map(|i| (format!("old-{i}"), 1)));
    write_durable(&ledger_path(root), &serde_json::to_vec(&ledger).unwrap()).unwrap();
    let mut limits = ReadLimits::explicit_read();
    limits.max_total_bytes = 16 * 1024;
    let small = ReceiptInspector::new(&ctx, &limits);
    assert_eq!(small.lookup("bob", "m1"), ReceiptLookup::Unknown);
    assert!(small.partial());
}

// trace:BUG-2641 | ai:codex
#[test]
fn bug2641_over_32_mib_history_keeps_unread_bodies_and_durable_ack() {
    let dir = repo();
    let root = dir.path();
    let mut first = message("000-unread");
    first.body = format!("000-unread: {}", "complete unread body ".repeat(100));
    write_local_message(root, &first).unwrap();
    // Archived history still occupies real payload bytes (gc does not yet
    // compact it). Insert the second unread message only after the history
    // has already exceeded the old byte ceiling.
    for n in 0..70 {
        let mut history = message(&format!("history-{n:03}"));
        history.archived = true;
        history.body = "h".repeat(512 * 1024);
        write_local_message(root, &history).unwrap();
    }
    let mut last = message("zzz-unread");
    last.body = format!("zzz-unread: {}", "complete unread body ".repeat(100));
    write_local_message(root, &last).unwrap();
    let (ctx, snap) = snapshot(root);
    assert!(snap.history_truncated);
    assert!(!snap.complete(), "history omission stays visible");
    assert!(snap.ack_eligible(), "discovery was complete");
    let unread: Vec<_> = snap
        .records
        .iter()
        .filter(|r| !r.message.archived)
        .collect();
    assert_eq!(unread.len(), 2);
    for r in &unread {
        assert_eq!(
            r.message.body,
            format!("{}: {}", r.message.id, "complete unread body ".repeat(100))
        );
        assert!(snap.record_ack_complete(r));
    }
    let ack = AckPlan::prepare(&snap, ctx.repository().map(|r| r.path()), &bob(), &unread)
        .unwrap()
        .unwrap();
    assert!(
        !ledger_path(root).exists(),
        "preparation never acknowledges"
    );
    ack.commit(&bob()).unwrap();
    let (_, again) = snapshot(root);
    for id in ["000-unread", "zzz-unread"] {
        assert!(ledger_holds(root, "bob", id));
        assert!(matches!(
            again
                .records
                .iter()
                .find(|r| r.message.id == id)
                .unwrap()
                .read,
            ReadEvidence::Acknowledged {
                origin: AckOrigin::Anchor { .. },
                ..
            }
        ));
    }
}

// trace:BUG-2641 | ai:codex
#[test]
fn bug2641_discovery_exhaustion_and_uncertified_records_refuse_ack() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    let (ctx, snap) = snapshot(root);
    let mut altered = snap.records[0].clone();
    altered.message.body.push_str("not discovered");
    assert!(matches!(
        AckPlan::prepare(
            &snap,
            ctx.repository().map(|r| r.path()),
            &bob(),
            &[&altered]
        ),
        Err(AckError::Partial)
    ));
    let mut limits = ReadLimits::explicit_read();
    limits.max_discovery_bytes = 1;
    let partial = logical_snapshot(&ctx, &bob(), &Query::Inbox, &limits);
    assert!(!partial.ack_eligible());
    assert!(!partial.record_ack_complete(&snap.records[0]));
    assert!(!ledger_path(root).exists());
}

// trace:BUG-2641 | ai:codex
#[test]
fn bug2641_history_omission_preserves_cohort_ownership_and_uuid_receipts() {
    use crate::mailbox_delivery::{DeliverySidecar, RecipientUuid};
    use crate::mailbox_identity::MailIdentityFacts;
    let dir = repo();
    let root = dir.path();
    let uuid = RecipientUuid::parse("00000000-0000-4000-8000-0000000000aa").unwrap();
    let facts = MailIdentityFacts::fixture(
        Some("bob"),
        Some("codex"),
        "joe",
        false,
        &[],
        Some("advisor"),
        None,
    )
    .with_recipient(uuid.clone());
    let audience = ResolvedAudience::from_facts(&facts);
    let mut history = message("history");
    history.archived = true;
    write_local_message(root, &history).unwrap();
    for id in ["hello-a", "hello-b", "hello-c"] {
        let mut mail = message(id);
        mail.to = Recipient::Agent("advisor".into());
        let sidecar = DeliverySidecar::cohort(id, "advisor", 1, &[uuid.clone()]);
        crate::mailbox::write_local_message_with_deliveries(root, &mail, Some(&sidecar)).unwrap();
    }
    let ctx = MailboxContext::resolve(root, None).unwrap();
    let mut limits = ReadLimits::explicit_read();
    limits.max_total_bytes = 1;
    let snap = logical_snapshot(&ctx, &audience, &Query::Inbox, &limits);
    assert!(snap.history_truncated);
    let shown: Vec<_> = snap.records.iter().collect();
    assert_eq!(shown.len(), 3);
    for record in &shown {
        assert_eq!(record.delivered_to.as_ref(), Some(&uuid));
        assert!(snap.record_ack_complete(record));
    }
    AckPlan::prepare(&snap, ctx.repository().map(|r| r.path()), &audience, &shown)
        .unwrap()
        .unwrap()
        .commit(&audience)
        .unwrap();
    let again = logical_snapshot(&ctx, &audience, &Query::Inbox, &ReadLimits::explicit_read());
    for record in again.records.iter().filter(|r| !r.message.archived) {
        assert!(ledger_holds(root, &uuid.receipt_key(), &record.message.id));
        assert!(!ledger_holds(root, "advisor", &record.message.id));
        assert!(matches!(record.read, ReadEvidence::Acknowledged { .. }));
    }
}

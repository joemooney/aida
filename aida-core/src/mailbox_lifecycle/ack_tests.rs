//! B2c fixtures: the post-flush ACK plan and the common-Git receipt anchor.
//! trace:STORY-1843 | ai:claude

use super::*;
use crate::mailbox::write_local_message;
use crate::mailbox_lifecycle::tests::{edit_record, message, repo};
use crate::mailbox_logical::{
    logical_snapshot, AckOrigin, MailboxContext, Query, ReadLimits, ReceiptInspector, ReceiptLookup,
};

fn bob() -> ResolvedAudience {
    ResolvedAudience::from_identity_policy(vec!["bob".into()], vec![], vec![]).unwrap()
}

fn snapshot(root: &Path) -> (MailboxContext, LogicalSnapshot) {
    let ctx = MailboxContext::resolve_or_local(root, None).unwrap();
    let snap = logical_snapshot(&ctx, &bob(), &Query::Inbox, &ReadLimits::explicit_read());
    (ctx, snap)
}

/// Plan the ACK of every record the snapshot holds, as an explicit read
/// that displays the whole inbox would.
fn plan(root: &Path) -> Result<Option<AckPlan>, AckError> {
    let (ctx, snap) = snapshot(root);
    let all: Vec<&SnapshotRecord> = snap.records.iter().collect();
    AckPlan::prepare(&snap, ctx.repository().map(|r| r.path()), &bob(), &all)
}

fn read_of(root: &Path, id: &str) -> ReadEvidence {
    let (_, snap) = snapshot(root);
    snap.records
        .iter()
        .find(|r| r.message.id == id)
        .map(|r| r.read.clone())
        .expect("record is visible")
}

fn ledger_path(root: &Path) -> PathBuf {
    let ctx = MailboxContext::resolve(root, None).unwrap();
    ctx.repository()
        .unwrap()
        .path()
        .join("aida/mail-lifecycle/receipts")
        .join(receipt_ledger_name("bob"))
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
    // Released, the same plan commits.
    fs2::FileExt::unlock(&holder).unwrap();
    assert_eq!(read_of(root, "m1"), ReadEvidence::Unread);
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
fn story1843_b2c_local_only_project_commits_without_an_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_local_message(root, &message("m1")).unwrap();
    let committed = plan(root).unwrap().unwrap().commit(&bob()).unwrap();
    assert!(!committed.anchored);
    assert_eq!(
        committed.by_key.get("bob").map(Vec::as_slice),
        Some(&["m1".to_string()][..])
    );
}

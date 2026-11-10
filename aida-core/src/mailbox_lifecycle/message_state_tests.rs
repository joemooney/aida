//! Message-state retirement, independent of an activation's build identity.
//! trace:TASK-3420 | ai:codex

use super::retire::{retire_source, Point, RefusalReason, HOOK_AT};
use super::tests::{assert_open, mailbox, message, opts, refusal, repo, retired, source};
use super::*;
use crate::mailbox::write_local_message;
use crate::mailbox_delivery::DeliverySidecar;
use crate::mailbox_logical::{
    logical_snapshot, MailboxContext, Query, ReadLimits, ResolvedAudience,
};

pub(super) fn preserve_canonical(root: &Path) -> PathBuf {
    preserve_mail_at(root, &mailbox(root))
}

pub(super) fn preserve_mail_at(root: &Path, mail: &Path) -> PathBuf {
    let common = source(root)
        .anchor
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let store = common.join("retention-test-store");
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&common)
            .arg("--work-tree")
            .arg(common.parent().unwrap())
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    if !store.exists() {
        git(&["commit", "--allow-empty", "-qm", "seed"]);
        git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "aida-store",
            store.to_str().unwrap(),
        ]);
    }
    std::fs::create_dir_all(store.join("mailbox")).unwrap();
    for entry in std::fs::read_dir(mail).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json")
            && !path
                .file_name()
                .unwrap()
                .as_encoded_bytes()
                .starts_with(b".")
        {
            let bytes = std::fs::read(&path).unwrap();
            crate::write_atomic(
                &store.join("mailbox").join(path.file_name().unwrap()),
                &bytes,
            )
            .unwrap();
        }
    }
    store
}

fn seed() -> tempfile::TempDir {
    let root = repo();
    super::activation::FRESH_SCAN.with(|f| f.set(true));
    write_local_message(root.path(), &message("unacked")).unwrap();
    root
}

#[test]
fn task3420_unacked_preserved_retires_and_remains_readable_without_activation() {
    let root = seed();
    let store = preserve_canonical(root.path());
    assert!(!source(root.path()).anchor.join(ACTIVATION_FILE).exists());
    let (_, retained) = retired(retire_source(root.path(), &opts()).unwrap());
    assert!(retained.join("data/unacked.json").exists());
    let ctx = MailboxContext::resolve(root.path(), Some(&store)).unwrap();
    let audience =
        ResolvedAudience::from_identity_policy(vec!["bob".into()], vec![], vec![]).unwrap();
    let snapshot = logical_snapshot(&ctx, &audience, &Query::Inbox, &ReadLimits::explicit_read());
    assert!(snapshot.complete(), "{snapshot:?}");
    assert_eq!(snapshot.records.len(), 1);
    assert_eq!(snapshot.records[0].message.id, "unacked");
    assert_eq!(
        snapshot.records[0].read,
        crate::mailbox_logical::ReadEvidence::Unread
    );
}

#[test]
fn task3420_unacked_unpreserved_retains() {
    let root = seed();
    let (reason, _) = refusal(retire_source(root.path(), &opts()).unwrap());
    assert!(
        matches!(reason, RefusalReason::MessageState { .. }),
        "{reason}"
    );
    assert!(mailbox(root.path()).join("unacked.json").exists());
    assert_open(root.path());
}

#[test]
fn task3420_queued_retains_even_with_a_canonical_copy() {
    let root = repo();
    super::activation::FRESH_SCAN.with(|f| f.set(true));
    let msg = message("queued");
    crate::mailbox::write_local_message_with_deliveries(
        root.path(),
        &msg,
        Some(&DeliverySidecar::queued("queued", "reviewer", 1)),
    )
    .unwrap();
    preserve_canonical(root.path());
    let (reason, _) = refusal(retire_source(root.path(), &opts()).unwrap());
    assert!(
        matches!(reason, RefusalReason::MessageState { .. }),
        "{reason}"
    );
    assert!(mailbox(root.path()).join("queued.json").exists());
    assert_open(root.path());
}

#[test]
fn task3420_writer_appearing_after_fencing_is_caught_even_with_current_activation() {
    let root = seed();
    preserve_canonical(root.path());
    super::tests::activate(root.path());
    HOOK_AT.with(|h| {
        *h.borrow_mut() = Some((
            Point::AfterFence,
            Box::new(|| {
                super::activation::FRESH_SCAN.with(|f| f.set(false));
            }),
        ))
    });
    let (reason, _) = refusal(retire_source(root.path(), &opts()).unwrap());
    assert_eq!(reason, RefusalReason::WriterAdmissionUnknown);
    assert!(mailbox(root.path()).join("unacked.json").exists());
    assert_open(root.path());
}

#[test]
fn task3420_same_id_with_different_canonical_payload_retains() {
    let root = seed();
    let store = preserve_canonical(root.path());
    let mut other = message("unacked");
    other.body = "different".into();
    crate::write_atomic(
        &store.join("mailbox/unacked.json"),
        &serde_json::to_vec(&other).unwrap(),
    )
    .unwrap();
    let (reason, _) = refusal(retire_source(root.path(), &opts()).unwrap());
    assert!(matches!(reason, RefusalReason::MessageState { .. }));
    assert_open(root.path());
}

#[test]
fn task3420_bound_queued_delivery_can_retire_once_preserved() {
    use crate::mailbox_delivery::{bind_queued_delivery, RecipientUuid};
    let root = repo();
    let msg = message("bound");
    let sidecar = DeliverySidecar::queued("bound", "reviewer", 1);
    crate::mailbox::write_local_message_with_deliveries(root.path(), &msg, Some(&sidecar)).unwrap();
    preserve_canonical(root.path());
    let recipient = RecipientUuid::parse("00000000-0000-4000-8000-000000000001").unwrap();
    bind_queued_delivery(
        Some(&root.path().join(".git")),
        root.path(),
        sidecar.queued_ids().next().unwrap(),
        &recipient,
        || true,
        Duration::from_secs(1),
    )
    .unwrap();
    retired(retire_source(root.path(), &opts()).unwrap());
}

#[test]
fn task3420_unsafe_or_conflicting_canonical_replicas_retain() {
    let root = seed();
    let store = preserve_canonical(root.path());
    let path = store.join("mailbox/unacked.json");
    let duplicate = store.join("mailbox/duplicate.json");
    std::fs::copy(&path, &duplicate).unwrap();
    let (reason, _) = refusal(retire_source(root.path(), &opts()).unwrap());
    assert!(matches!(reason, RefusalReason::MessageState { .. }));
    assert_open(root.path());
    std::fs::remove_file(&duplicate).unwrap();
    std::fs::rename(&path, &duplicate).unwrap();
    std::os::unix::fs::symlink(&duplicate, &path).unwrap();
    let (reason, _) = refusal(retire_source(root.path(), &opts()).unwrap());
    assert!(matches!(reason, RefusalReason::MessageState { .. }));
    assert_open(root.path());
}

//! STORY-1843 D2b: relay run ownership in the logical view. Mail to a relay
//! label is owned by its run; a handover adds an owner without touching the
//! original delivery or its receipts; the run inspection counts what the
//! run never read. Real common-Git repository (main and a sibling).
// trace:STORY-1843 | ai:claude
use super::*;
use crate::mailbox_delivery::{
    handover_file_name, handover_record, parse_handover, write_handover, DeliverySidecar,
    RecipientUuid,
};
use crate::mailbox_identity::MailIdentityFacts;
use crate::mailbox_lifecycle::ack::AckPlan;
use std::path::{Path, PathBuf};
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

struct Repo {
    _dir: tempfile::TempDir,
    main: PathBuf,
    sib: PathBuf,
}

fn repo() -> Repo {
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
        &["worktree", "add", "-q", "-b", "s", sib.to_str().unwrap()],
    );
    Repo {
        _dir: dir,
        main,
        sib,
    }
}

const LABEL: &str = "relay-a";

fn run(n: u8) -> RecipientUuid {
    RecipientUuid::parse(&format!("00000000-0000-4000-8000-0000000000{n:02x}")).unwrap()
}

/// A session of relay run `n`: it holds the label and the run's UUID.
fn session(n: u8) -> ResolvedAudience {
    let facts =
        MailIdentityFacts::fixture(Some(LABEL), Some("claude"), "joe", false, &[], None, None)
            .with_recipient(run(n));
    ResolvedAudience::from_facts(&facts)
}

fn message(id: &str) -> Message {
    serde_json::from_value(serde_json::json!({
        "id": id, "thread_id": id, "from": "sender-x",
        "to": {"kind": "agent", "agent": LABEL},
        "timestamp": 10, "body": format!("body of {id}"), "subject": format!("about {id}"),
    }))
    .unwrap()
}

/// Send `id` to the label from `root`; returns its delivery id when it is
/// owned by run `owner`.
fn send(root: &Path, id: &str, sidecar: Option<DeliverySidecar>) -> Option<String> {
    let delivery = sidecar
        .as_ref()
        .map(|s| s.deliveries[0].delivery_id.clone());
    crate::mailbox::write_local_message_with_deliveries(root, &message(id), sidecar.as_ref())
        .unwrap();
    delivery
}

fn snap(from: &Path, audience: &ResolvedAudience) -> (MailboxContext, LogicalSnapshot) {
    let ctx = MailboxContext::resolve_or_local(from, None).unwrap();
    let s = logical_snapshot(&ctx, audience, &Query::Inbox, &ReadLimits::explicit_read());
    (ctx, s)
}

fn read(s: &LogicalSnapshot, id: &str) -> Option<ReadEvidence> {
    s.records
        .iter()
        .find(|r| r.message.id == id)
        .map(|r| r.read.clone())
}

fn ack(from: &Path, audience: &ResolvedAudience, id: &str) {
    let (ctx, s) = snap(from, audience);
    let records: Vec<_> = s.records.iter().filter(|r| r.message.id == id).collect();
    assert_eq!(records.len(), 1, "{id} not visible");
    let common = ctx.repository().map(|c| c.path().to_path_buf());
    AckPlan::prepare(&s, common.as_deref(), audience, &records)
        .unwrap()
        .unwrap()
        .commit(audience)
        .unwrap();
}

fn unread(from: &Path, n: u8) -> RelayRunUnread {
    let ctx = MailboxContext::resolve_or_local(from, None).unwrap();
    relay_run_unread(&ctx, LABEL, &run(n), &ReadLimits::explicit_read())
}

#[test]
fn relay_mail_is_owned_by_its_run_and_acknowledged_under_the_run_uuid() {
    let r = repo();
    send(&r.sib, "m1", Some(DeliverySidecar::relay("m1", &run(1))));
    let (_, s) = snap(&r.main, &session(1));
    let record = s.records.iter().find(|r| r.message.id == "m1").unwrap();
    assert_eq!(record.delivered_to.as_ref(), Some(&run(1)));
    let owned = record.owned_delivery.as_ref().unwrap();
    assert_eq!(owned.run_uuid.as_deref(), Some(run(1).as_str()));
    // A later run holding the same label does not see the old run's mail.
    assert_eq!(read(&snap(&r.main, &session(2)).1, "m1"), None);
    ack(&r.main, &session(1), "m1");
    assert!(matches!(
        read(&snap(&r.sib, &session(1)).1, "m1"),
        Some(ReadEvidence::Acknowledged { ref key, .. }) if *key == run(1).receipt_key()
    ));
}

#[test]
fn handover_adds_an_owner_and_keeps_the_old_runs_receipts() {
    let r = repo();
    send(
        &r.main,
        "read",
        Some(DeliverySidecar::relay("read", &run(1))),
    );
    let unread_delivery = send(
        &r.sib,
        "unread",
        Some(DeliverySidecar::relay("unread", &run(1))),
    )
    .unwrap();
    ack(&r.main, &session(1), "read");
    assert_eq!(
        unread(&r.main, 1),
        RelayRunUnread {
            owned: 1,
            legacy_label: 0,
            complete: true
        }
    );

    // The handover is written by the new run's checkout, beside nothing.
    write_handover(
        &r.main,
        &handover_record("unread", &unread_delivery, &run(2)),
    )
    .unwrap();
    let (_, new_view) = snap(&r.main, &session(2));
    let ids: Vec<_> = new_view
        .records
        .iter()
        .map(|r| r.message.id.as_str())
        .collect();
    assert_eq!(ids, ["unread"], "only the handed-over delivery moves");
    assert_eq!(
        new_view.records[0]
            .owned_delivery
            .as_ref()
            .unwrap()
            .handover_of
            .as_deref(),
        Some(unread_delivery.as_str())
    );
    assert_eq!(unread(&r.main, 2).owned, 1);

    // The new run acknowledges under its own UUID; the old run's evidence
    // is untouched: its read message stays read, its unread one unread.
    ack(&r.main, &session(2), "unread");
    assert!(matches!(
        read(&snap(&r.main, &session(2)).1, "unread"),
        Some(ReadEvidence::Acknowledged { ref key, .. }) if *key == run(2).receipt_key()
    ));
    let (_, old_view) = snap(&r.main, &session(1));
    assert_eq!(read(&old_view, "unread"), Some(ReadEvidence::Unread));
    assert!(matches!(
        read(&old_view, "read"),
        Some(ReadEvidence::Acknowledged { ref key, .. }) if *key == run(1).receipt_key()
    ));
    assert_eq!(unread(&r.main, 1).owned, 1);
    assert_eq!(unread(&r.main, 2).owned, 0);
}

#[test]
fn a_handover_of_an_unknown_delivery_owns_nothing() {
    let r = repo();
    send(&r.main, "m", Some(DeliverySidecar::relay("m", &run(1))));
    write_handover(&r.main, &handover_record("m", "not-a-delivery", &run(2))).unwrap();
    assert_eq!(read(&snap(&r.main, &session(2)).1, "m"), None);
    assert_eq!(unread(&r.main, 2).owned, 0);
}

#[test]
fn the_inspection_counts_legacy_label_mail_and_flags_notices() {
    let r = repo();
    send(&r.main, "legacy", None);
    send(
        &r.main,
        "owned",
        Some(DeliverySidecar::relay("owned", &run(1))),
    );
    send(
        &r.main,
        "bounce",
        Some(DeliverySidecar::notice(
            "bounce",
            &run(1),
            "earlier-delivery",
        )),
    );
    let ctx = MailboxContext::resolve_or_local(&r.main, None).unwrap();
    let found = relay_run_unread_deliveries(&ctx, LABEL, &run(1), &ReadLimits::explicit_read());
    assert!(found.complete);
    assert_eq!(found.legacy_label, 1);
    let mut owned: Vec<_> = found
        .unread
        .iter()
        .map(|d| (d.message_id.as_str(), d.notice))
        .collect();
    owned.sort();
    assert_eq!(owned, [("bounce", true), ("owned", false)]);
    assert_eq!(found.unread[0].from, "sender-x");
    let counts = unread(&r.main, 1);
    assert_eq!((counts.owned, counts.legacy_label), (2, 1));
    assert!(counts.any_or_unknown());
}

#[test]
fn handover_and_notice_markers_are_validated() {
    let record = handover_record("m", "d-1", &run(2));
    let bytes = serde_json::to_vec(&record).unwrap();
    assert!(parse_handover(&bytes, &handover_file_name("m", &run(2))).is_ok());
    assert!(parse_handover(&bytes, &handover_file_name("m", &run(3))).is_err());
    // A sidecar never carries a handover record.
    let mut sidecar = DeliverySidecar::relay("m", &run(1));
    sidecar.deliveries[0].handover_of = Some("d-1".into());
    let bytes = serde_json::to_vec(&sidecar).unwrap();
    assert!(DeliverySidecar::parse(&bytes, "m").is_err());
    // The same handover written twice is one record.
    assert_eq!(
        handover_record("m", "d-1", &run(2)),
        handover_record("m", "d-1", &run(2))
    );
}

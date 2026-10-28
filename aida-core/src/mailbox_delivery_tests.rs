//! STORY-1843 D1: role-cohort delivery ownership, per-UUID receipts and the
//! queued-for-role binding, against a real common-Git repository (main, a
//! sibling worktree and the store worktree). No real mailboxes or vendors.
// trace:STORY-1843 | ai:claude
use super::*;
use crate::mailbox_identity::MailIdentityFacts;
use crate::mailbox_lifecycle::ack::AckPlan;
use crate::mailbox_logical::{
    logical_snapshot, LogicalSnapshot, MailboxContext, Query, ReadEvidence, ReadLimits,
    ResolvedAudience,
};
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

fn uuid(n: u8) -> RecipientUuid {
    RecipientUuid::parse(&format!("00000000-0000-4000-8000-0000000000{n:02x}")).unwrap()
}

/// A reviewer session with recipient `n` (`None`: a UUID-less reviewer).
fn reviewer(handle: &str, n: Option<u8>) -> ResolvedAudience {
    let facts = MailIdentityFacts::fixture(
        Some(handle),
        Some("codex"),
        "joe",
        false,
        &[],
        Some("reviewer"),
        None,
    );
    let facts = match n {
        Some(n) => facts.with_recipient(uuid(n)),
        None => facts,
    };
    ResolvedAudience::from_facts(&facts)
}

fn role_message(id: &str, ts: i64) -> crate::mailbox::Message {
    serde_json::from_value(serde_json::json!({
        "id": id, "thread_id": id, "from": "sender",
        "to": {"kind": "agent", "agent": "reviewer"},
        "timestamp": ts, "body": format!("body of {id}"),
    }))
    .unwrap()
}

fn send(root: &Path, id: &str, ts: i64, sidecar: Option<&DeliverySidecar>) {
    crate::mailbox::write_local_message_with_deliveries(root, &role_message(id, ts), sidecar)
        .unwrap();
}

fn snap(from: &Path, audience: &ResolvedAudience) -> (MailboxContext, LogicalSnapshot) {
    let ctx = MailboxContext::resolve_or_local(from, None).unwrap();
    let s = logical_snapshot(&ctx, audience, &Query::Inbox, &ReadLimits::explicit_read());
    (ctx, s)
}

fn visible(from: &Path, audience: &ResolvedAudience) -> Vec<String> {
    snap(from, audience)
        .1
        .records
        .iter()
        .map(|r| r.message.id.clone())
        .collect()
}

#[test]
fn recipient_uuid_accepts_only_the_canonical_form() {
    assert!(RecipientUuid::parse("0a1b2c3d-0000-4000-8000-00000000abcd").is_some());
    for bad in [
        "",
        "not-a-uuid",
        "0A1B2C3D-0000-4000-8000-00000000ABCD",
        "0a1b2c3d00004000800000000000abcd",
        "0a1b2c3d-0000-4000-8000-00000000abc",
        "../0000-4000-8000-00000000abcd",
    ] {
        assert!(RecipientUuid::parse(bad).is_none(), "{bad}");
        let json = serde_json::to_string(bad).unwrap();
        assert!(
            serde_json::from_str::<RecipientUuid>(&json).is_err(),
            "{bad}"
        );
    }
    assert_eq!(uuid(1).receipt_key(), format!("uuid:{}", uuid(1).as_str()));
}

#[test]
fn sidecar_parse_validates_its_message_and_records() {
    let sidecar = DeliverySidecar::cohort("m1", "reviewer", 1, &[uuid(1)]);
    let bytes = serde_json::to_vec(&sidecar).unwrap();
    assert_eq!(DeliverySidecar::parse(&bytes, "m1").unwrap(), sidecar);
    assert!(DeliverySidecar::parse(&bytes, "m2").is_err());
    let mut empty = sidecar.clone();
    empty.deliveries.clear();
    assert!(DeliverySidecar::parse(&serde_json::to_vec(&empty).unwrap(), "m1").is_err());
    let mut ownerless = sidecar;
    ownerless.deliveries[0].recipient_uuid = None;
    assert!(DeliverySidecar::parse(&serde_json::to_vec(&ownerless).unwrap(), "m1").is_err());
    // A sidecar for another message is refused before anything is written.
    let tmp = tempfile::tempdir().unwrap();
    let other = DeliverySidecar::cohort("other", "reviewer", 1, &[uuid(1)]);
    assert!(crate::mailbox::write_local_message_with_deliveries(
        tmp.path(),
        &role_message("m1", 1),
        Some(&other)
    )
    .is_err());
    assert!(!tmp.path().join(".aida").exists());
}

#[test]
fn story1843_d1_cohort_is_visible_only_to_its_snapshot_and_legacy_is_unchanged() {
    let r = repo();
    // Zero: a legacy role message (no sidecar). One: a cohort of A. Many: A and B.
    send(&r.sib, "legacy", 10, None);
    send(
        &r.sib,
        "one",
        20,
        Some(&DeliverySidecar::cohort("one", "reviewer", 20, &[uuid(1)])),
    );
    send(
        &r.sib,
        "many",
        30,
        Some(&DeliverySidecar::cohort(
            "many",
            "reviewer",
            30,
            &[uuid(1), uuid(2)],
        )),
    );
    let a = reviewer("rev-a", Some(1));
    let b = reviewer("rev-b", Some(2));
    let late = reviewer("rev-late", Some(3));
    let uuidless = reviewer("rev-old", None);
    assert_eq!(visible(&r.main, &a), ["legacy", "one", "many"]);
    assert_eq!(visible(&r.main, &b), ["legacy", "many"]);
    assert_eq!(visible(&r.main, &late), ["legacy"]);
    assert_eq!(visible(&r.main, &uuidless), ["legacy"]);
    let (_, s) = snap(&r.main, &a);
    let owned: Vec<_> = s.records.iter().map(|r| r.delivered_to.clone()).collect();
    assert_eq!(owned, [None, Some(uuid(1)), Some(uuid(1))]);
    assert!(s.complete());
    // A narrowed audience without the role sees no cohort mail.
    let handle_only = a.narrowed_to("rev-a").unwrap();
    assert!(visible(&r.main, &handle_only).is_empty());
}

#[test]
fn story1843_d1_cohort_receipts_are_independent_and_ignore_role_watermarks() {
    let r = repo();
    send(&r.main, "legacy", 10, None);
    send(
        &r.main,
        "many",
        30,
        Some(&DeliverySidecar::cohort(
            "many",
            "reviewer",
            30,
            &[uuid(1), uuid(2)],
        )),
    );
    // An old shared role watermark covers both by timestamp.
    let marks = r.main.join(".aida/mailbox/.read");
    std::fs::create_dir_all(&marks).unwrap();
    std::fs::write(marks.join("reviewer.txt"), "1000").unwrap();
    let a = reviewer("rev-a", Some(1));
    let b = reviewer("rev-b", Some(2));
    let (ctx, s) = snap(&r.main, &a);
    let read = |s: &LogicalSnapshot, id: &str| {
        s.records
            .iter()
            .find(|r| r.message.id == id)
            .unwrap()
            .read
            .clone()
    };
    assert!(matches!(
        read(&s, "legacy"),
        ReadEvidence::LegacyWatermark { .. }
    ));
    assert_eq!(
        read(&s, "many"),
        ReadEvidence::Unread,
        "a role watermark is not a UUID read"
    );

    // A acknowledges the cohort message under its own UUID key only.
    let many: Vec<_> = s
        .records
        .iter()
        .filter(|r| r.message.id == "many")
        .collect();
    let common = ctx.repository().map(|c| c.path().to_path_buf());
    let plan = AckPlan::prepare(&s, common.as_deref(), &a, &many)
        .unwrap()
        .unwrap();
    assert_eq!(
        plan.by_key().keys().cloned().collect::<Vec<_>>(),
        [uuid(1).receipt_key()]
    );
    // Another reader's audience cannot commit A's UUID key.
    assert!(plan.clone().commit(&b).is_err());
    plan.commit(&a).unwrap();

    let (_, after_a) = snap(&r.main, &a);
    assert!(
        matches!(read(&after_a, "many"), ReadEvidence::Acknowledged { ref key, .. } if *key == uuid(1).receipt_key())
    );
    let (_, after_b) = snap(&r.sib, &b);
    assert_eq!(
        read(&after_b, "many"),
        ReadEvidence::Unread,
        "B's receipt is its own"
    );
}

/// Queue one message for the reviewer role in `root`; returns its delivery id.
fn queue(root: &Path, id: &str) -> String {
    let sidecar = DeliverySidecar::queued(id, "reviewer", 5);
    let delivery = sidecar.deliveries[0].delivery_id.clone();
    send(root, id, 5, Some(&sidecar));
    delivery
}

fn race(r_main: &Path, delivery: &str) -> Vec<BindOutcome> {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = [1u8, 2]
        .into_iter()
        .map(|n| {
            let barrier = barrier.clone();
            let root = r_main.to_path_buf();
            let delivery = delivery.to_string();
            std::thread::spawn(move || {
                let ctx = MailboxContext::resolve_or_local(&root, None).unwrap();
                barrier.wait();
                bind_queued_delivery(
                    ctx.repository().map(|c| c.path()),
                    ctx.requested_checkout().path(),
                    &delivery,
                    &uuid(n),
                    || true,
                    std::time::Duration::from_secs(5),
                )
                .unwrap()
            })
        })
        .collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
}

fn assert_one_winner(outcomes: &[BindOutcome]) -> RecipientUuid {
    let bound = outcomes
        .iter()
        .filter(|o| **o == BindOutcome::Bound)
        .count();
    assert_eq!(bound, 1, "exactly one binder wins: {outcomes:?}");
    let winner = if outcomes[0] == BindOutcome::Bound {
        uuid(1)
    } else {
        uuid(2)
    };
    assert!(outcomes.contains(&BindOutcome::AlreadyBound(winner.clone())));
    winner
}

#[test]
fn story1843_d1_queued_delivery_binds_exactly_once_under_a_two_reader_race() {
    let r = repo();
    let delivery = queue(&r.sib, "queued");
    let a = reviewer("rev-a", Some(1));
    let b = reviewer("rev-b", Some(2));
    for audience in [&a, &b] {
        let (_, s) = snap(&r.main, audience);
        assert!(
            s.records.is_empty(),
            "a queued message is hidden until bound"
        );
        assert_eq!(s.pending_deliveries.len(), 1);
        assert_eq!(s.pending_deliveries[0].delivery_id, delivery);
    }
    // The UUID-less reader is not eligible.
    assert!(snap(&r.main, &reviewer("rev-old", None))
        .1
        .pending_deliveries
        .is_empty());

    let winner = assert_one_winner(&race(&r.main, &delivery));
    let (winner_aud, loser_aud) = if winner == uuid(1) {
        (&a, &b)
    } else {
        (&b, &a)
    };
    let (_, won) = snap(&r.sib, winner_aud);
    assert_eq!(won.records.len(), 1);
    assert_eq!(won.records[0].delivered_to, Some(winner.clone()));
    assert!(won.pending_deliveries.is_empty());
    let (_, lost) = snap(&r.main, loser_aud);
    assert!(lost.records.is_empty() && lost.pending_deliveries.is_empty());
    // The binding lives in the private anchor, not in either worktree.
    let common = MailboxContext::resolve(&r.main, None)
        .unwrap()
        .repository()
        .unwrap()
        .path()
        .to_path_buf();
    let bound = common
        .join("aida/mail-lifecycle/deliveries/bound")
        .join(binding_file_name(&delivery));
    assert!(bound.is_file());
}

#[test]
fn story1843_d1_local_only_project_binds_once_with_the_same_rule() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let delivery = queue(&root, "queued");
    let winner = assert_one_winner(&race(&root, &delivery));
    let audience = reviewer("rev", Some(if winner == uuid(1) { 1 } else { 2 }));
    assert_eq!(visible(&root, &audience), ["queued"]);
    assert!(root
        .join(".aida/mailbox/.deliveries/bound")
        .join(binding_file_name(&delivery))
        .is_file());
}

#[test]
fn story1843_d1_bind_rechecks_ownership_and_writes_nothing_when_lost() {
    let r = repo();
    let delivery = queue(&r.main, "queued");
    let ctx = MailboxContext::resolve(&r.main, None).unwrap();
    let common = ctx.repository().map(|c| c.path());
    let refused = bind_queued_delivery(
        common,
        &r.main,
        &delivery,
        &uuid(1),
        || false,
        std::time::Duration::from_secs(1),
    );
    assert!(matches!(refused, Err(BindError::NotOwned)));
    assert_eq!(
        snap(&r.main, &reviewer("rev-a", Some(1)))
            .1
            .pending_deliveries
            .len(),
        1
    );
}

// A symlinked `.deliveries` or `.deliveries/bound` is refused, never
// followed: nothing lands at the link's target and the message is not
// written without its ownership.
// trace:STORY-1843 | ai:claude
#[test]
fn story1843_d1_symlinked_delivery_dirs_are_refused_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap().join("proj");
    let outside = root.with_file_name("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let mailbox = root.join(".aida/mailbox");
    std::fs::create_dir_all(&mailbox).unwrap();
    std::os::unix::fs::symlink(&outside, mailbox.join(DELIVERIES_DIR)).unwrap();
    let sidecar = DeliverySidecar::cohort("m1", "reviewer", 1, &[uuid(1)]);
    let err = crate::mailbox::write_local_message_with_deliveries(
        &root,
        &role_message("m1", 1),
        Some(&sidecar),
    )
    .unwrap_err();
    assert!(err.to_string().contains("refusing to follow"), "{err}");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    assert!(!mailbox.join("m1.json").exists());

    // The bindings directory of a local-only project.
    std::fs::remove_file(mailbox.join(DELIVERIES_DIR)).unwrap();
    std::fs::create_dir(mailbox.join(DELIVERIES_DIR)).unwrap();
    std::os::unix::fs::symlink(&outside, mailbox.join(DELIVERIES_DIR).join(BINDINGS_DIR)).unwrap();
    let refused = bind_queued_delivery(
        None,
        &root,
        "d1",
        &uuid(1),
        || true,
        std::time::Duration::from_secs(1),
    );
    assert!(matches!(refused, Err(BindError::Io(_))), "{refused:?}");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

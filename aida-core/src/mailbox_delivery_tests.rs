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

fn common_of(root: &Path) -> PathBuf {
    MailboxContext::resolve(root, None)
        .unwrap()
        .repository()
        .unwrap()
        .path()
        .to_path_buf()
}

fn private_dir(path: &Path) {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .unwrap();
}

// A torn (empty) binding left by a writer that predates the link protocol
// makes only ITS delivery unknown: that message is hidden and not offered,
// the view stays complete, and another queued delivery still surfaces and
// binds. A new bind publishes by link, so its final file is never empty.
// trace:STORY-1843 | ai:claude
#[test]
fn story1843_d1_a_torn_binding_is_local_to_its_delivery() {
    let r = repo();
    let torn = queue(&r.sib, "torn");
    let fine = queue(&r.sib, "fine");
    let bound = common_of(&r.main).join("aida/mail-lifecycle/deliveries/bound");
    private_dir(&bound);
    std::fs::write(bound.join(binding_file_name(&torn)), b"").unwrap();

    let a = reviewer("rev-a", Some(1));
    let (ctx, s) = snap(&r.main, &a);
    assert!(s.complete(), "a torn binding never makes the view partial");
    // trace:BUG-b6b1278890774ef993d2ebc327862470-178 | ai:codex
    assert_eq!(s.inspection_unknown_ids, ["torn"]);

    assert!(s.records.is_empty());
    let offered: Vec<_> = s
        .pending_deliveries
        .iter()
        .map(|p| p.delivery_id.clone())
        .collect();
    assert_eq!(
        offered,
        [fine.clone()],
        "only the readable delivery is offered"
    );

    let common = ctx.repository().map(|c| c.path());
    let bind = |d: &str| {
        bind_queued_delivery(
            common,
            &r.main,
            d,
            &uuid(1),
            || true,
            std::time::Duration::from_secs(1),
        )
    };
    assert!(bind(&torn).is_err(), "the torn delivery stays unknown");
    assert_eq!(bind(&fine).unwrap(), BindOutcome::Bound);
    let published = std::fs::read(bound.join(binding_file_name(&fine))).unwrap();
    assert_eq!(
        DeliveryBinding::parse(&published, &fine)
            .unwrap()
            .recipient_uuid,
        uuid(1)
    );
    // No temp file is left behind.
    let names: Vec<_> = std::fs::read_dir(&bound)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
    assert_eq!(visible(&r.main, &a), ["fine"]);
}

// Retained sidecars are read by the ID of a message the view holds, never
// scanned: a corrupt one for an unrelated message is never read, and a
// corrupt one for a held role message hides only that message. The view
// stays complete and queued deliveries still surface.
// trace:STORY-1843 | ai:claude
#[test]
fn story1843_d1_retained_sidecars_are_looked_up_by_id_and_faults_stay_local() {
    let r = repo();
    let deliveries = common_of(&r.main).join("aida/mail-lifecycle/retained/k1/1/data/.deliveries");
    private_dir(&deliveries);
    std::fs::write(deliveries.join("unrelated.json"), b"{not json").unwrap();
    let queued = queue(&r.main, "queued");
    send(&r.main, "legacy", 10, None);
    let a = reviewer("rev-a", Some(1));
    let (_, s) = snap(&r.main, &a);
    assert!(
        s.complete(),
        "an unrelated corrupt retained sidecar is never read"
    );
    assert_eq!(visible(&r.main, &a), ["legacy"]);
    assert_eq!(s.pending_deliveries.len(), 1);
    assert_eq!(s.pending_deliveries[0].delivery_id, queued);

    // A corrupt retained sidecar for the held legacy message: only that
    // message's ownership is unknown, so only it is hidden.
    std::fs::write(deliveries.join("legacy.json"), b"{not json").unwrap();
    let (_, s) = snap(&r.main, &a);
    assert!(s.complete(), "the fault is local to its message");
    // trace:BUG-b6b1278890774ef993d2ebc327862470-178 | ai:codex
    assert_eq!(s.inspection_unknown_ids, ["legacy"]);

    assert!(s.records.is_empty());
    assert_eq!(s.pending_deliveries.len(), 1);

    // A valid retained sidecar gates the replica: an outsider cannot see it.
    std::fs::write(
        deliveries.join("legacy.json"),
        serde_json::to_vec(&DeliverySidecar::cohort(
            "legacy",
            "reviewer",
            10,
            &[uuid(1)],
        ))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(visible(&r.main, &a), ["legacy"]);
    assert!(visible(&r.main, &reviewer("rev-b", Some(2))).is_empty());
}

// Ruling F2, fail closed (narrowed): the cohort sidecar lives only in the
// sending source. When that live source's messages cannot be read but its
// `.deliveries/` can, the sidecar still gates the replica and legacy role
// mail is served. When its `.deliveries/` cannot be read either, no
// sidecar-less role message is served to anyone from that partial view,
// while direct mail stays visible.
// trace:STORY-1843 | ai:claude
#[test]
fn story1843_d1_an_unreadable_sending_source_fails_closed() {
    let r = repo();
    let sidecar = DeliverySidecar::cohort("cohort", "reviewer", 20, &[uuid(1)]);
    send(&r.sib, "cohort", 20, Some(&sidecar));
    // A sync or copy leaves a replica in main with no sidecar of its own.
    crate::mailbox::write_local_message(&r.main, &role_message("cohort", 20)).unwrap();
    send(&r.main, "legacy", 10, None);
    let direct: crate::mailbox::Message = serde_json::from_value(serde_json::json!({
        "id": "direct", "thread_id": "direct", "from": "sender",
        "to": {"kind": "agent", "agent": "rev-old"},
        "timestamp": 30, "body": "for one handle",
    }))
    .unwrap();
    crate::mailbox::write_local_message(&r.main, &direct).unwrap();
    // Vendor-group mail never carries a sidecar and is never gated.
    let vendor: crate::mailbox::Message = serde_json::from_value(serde_json::json!({
        "id": "vendor", "thread_id": "vendor", "from": "sender",
        "to": {"kind": "agent", "agent": "codex"},
        "timestamp": 40, "body": "for every codex session",
    }))
    .unwrap();
    crate::mailbox::write_local_message(&r.main, &vendor).unwrap();
    let member = reviewer("rev-a", Some(1));
    let uuidless = reviewer("rev-old", None);
    let late = reviewer("rev-late", Some(3));
    let served = |reader: &ResolvedAudience| {
        let (_, s) = snap(&r.main, reader);
        let ids: Vec<String> = s.records.iter().map(|r| r.message.id.clone()).collect();
        (s.complete(), ids)
    };
    assert_eq!(
        served(&uuidless),
        (
            true,
            vec!["legacy".into(), "direct".into(), "vendor".into()]
        )
    );
    assert_eq!(
        served(&late),
        (true, vec!["legacy".into(), "vendor".into()])
    );

    // A corrupt message: the source errors, its sidecars are still read.
    let bad = r.sib.join(".aida/mailbox/bad.json");
    std::fs::write(&bad, b"{not json").unwrap();
    assert_eq!(
        served(&uuidless),
        (
            false,
            vec!["legacy".into(), "direct".into(), "vendor".into()]
        )
    );
    assert_eq!(
        served(&late),
        (false, vec!["legacy".into(), "vendor".into()])
    );
    assert_eq!(
        served(&member),
        (
            false,
            vec!["legacy".into(), "cohort".into(), "vendor".into()]
        )
    );
    std::fs::remove_file(&bad).unwrap();

    // A corrupt sidecar: the source's `.deliveries/` cannot be read.
    let bad = r.sib.join(".aida/mailbox/.deliveries/bad.json");
    std::fs::write(&bad, b"{not json").unwrap();
    for reader in [&uuidless, &late, &member] {
        let (complete, ids) = served(reader);
        assert!(!complete, "the snapshot is partial");
        assert!(
            ids.iter().all(|id| id == "direct" || id == "vendor"),
            "no sidecar-less role mail is served: {ids:?}"
        );
        assert!(ids.iter().any(|id| id == "vendor"), "vendor mail is served");
    }
    std::fs::remove_file(&bad).unwrap();
    assert_eq!(
        served(&member),
        (
            true,
            vec!["legacy".into(), "cohort".into(), "vendor".into()]
        )
    );
}

// Ruling F2 (narrowed): a missing or prunable sibling worktree and a refused
// canonical store make the view partial, but cannot hold an unread sidecar
// (the retained lookup covers a retired source; the store never carries
// one), so legacy role mail in a healthy source is still served. Vendor
// group mail is never gated.
// trace:STORY-1843 | ai:claude
#[test]
fn story1843_d1_stale_worktrees_and_a_refused_store_keep_legacy_role_mail() {
    let r = repo();
    send(&r.main, "legacy", 10, None);
    let vendor: crate::mailbox::Message = serde_json::from_value(serde_json::json!({
        "id": "vendor", "thread_id": "vendor", "from": "sender",
        "to": {"kind": "agent", "agent": "codex"},
        "timestamp": 20, "body": "for every codex session",
    }))
    .unwrap();
    crate::mailbox::write_local_message(&r.main, &vendor).unwrap();
    // Locked, the deleted sibling is missing but not prunable, so inventory
    // takes its WorktreeMissing branch.
    git(&r.main, &["worktree", "lock", r.sib.to_str().unwrap()]);
    std::fs::remove_dir_all(&r.sib).unwrap();
    let foreign = r.main.parent().unwrap().join("foreign");
    std::fs::create_dir_all(&foreign).unwrap();
    git(&foreign, &["init", "-q", "-b", "main"]);
    for reader in [reviewer("rev-old", None), reviewer("rev-late", Some(3))] {
        let ctx = MailboxContext::resolve(&r.main, Some(&foreign)).unwrap();
        let s = logical_snapshot(&ctx, &reader, &Query::Inbox, &ReadLimits::explicit_read());
        assert!(!s.complete(), "the stale sibling and the refused store");
        assert!(
            s.sources.iter().any(|x| x.status
                == crate::mailbox_logical::SourceStatus::Error(
                    crate::mailbox_logical::SourceErrorCode::WorktreeMissing
                )),
            "{:?}",
            s.sources
        );
        let ids: Vec<_> = s.records.iter().map(|r| r.message.id.as_str()).collect();
        assert_eq!(ids, ["legacy", "vendor"]);
    }
}

// Ruling F2 (advisor, round 5): an inventory that cannot enumerate the live
// worktrees left their `.deliveries/` unread, so a sidecar-less role
// replica is gated for a UUID-less reader: when the inventory runs over its
// root budget, and when `git worktree list` fails.
// trace:STORY-1843 | ai:claude
#[test]
fn story1843_d1_an_incomplete_inventory_fails_closed() {
    let r = repo();
    let sidecar = DeliverySidecar::cohort("cohort", "reviewer", 20, &[uuid(1)]);
    send(&r.sib, "cohort", 20, Some(&sidecar));
    crate::mailbox::write_local_message(&r.main, &role_message("cohort", 20)).unwrap();
    seed_direct_and_vendor(&r.main);
    let uuidless = reviewer("rev-old", None);
    // Whole, the sibling's sidecar gates the replica.
    assert_eq!(visible(&r.main, &uuidless), ["direct", "vendor"]);

    let ctx = MailboxContext::resolve_or_local(&r.main, None).unwrap();
    let mut limits = ReadLimits::explicit_read();
    limits.max_roots = 1;
    let s = logical_snapshot(&ctx, &uuidless, &Query::Inbox, &limits);
    assert_eq!(
        s.inventory_error,
        Some(crate::mailbox_logical::SourceErrorCode::InventoryOverBudget)
    );
    assert_eq!(ids_of(&s), ["direct", "vendor"], "over budget");

    // A config git cannot parse makes `git worktree list` fail.
    let config = r.main.join(".git/config");
    let saved = std::fs::read(&config).unwrap();
    std::fs::write(&config, b"[[[not a config\n").unwrap();
    let s = logical_snapshot(&ctx, &uuidless, &Query::Inbox, &ReadLimits::explicit_read());
    std::fs::write(&config, saved).unwrap();
    assert_eq!(
        s.inventory_error,
        Some(crate::mailbox_logical::SourceErrorCode::InventoryFailed)
    );
    assert_eq!(ids_of(&s), ["direct", "vendor"], "inventory failed");
}

/// Direct mail to `rev-old` and vendor-group mail to `codex` in `root`:
/// neither can carry a sidecar, so neither is ever gated.
fn seed_direct_and_vendor(root: &Path) {
    for (id, to, ts) in [("direct", "rev-old", 30), ("vendor", "codex", 40)] {
        let m: crate::mailbox::Message = serde_json::from_value(serde_json::json!({
            "id": id, "thread_id": id, "from": "sender",
            "to": {"kind": "agent", "agent": to},
            "timestamp": ts, "body": format!("body of {id}"),
        }))
        .unwrap();
        crate::mailbox::write_local_message(root, &m).unwrap();
    }
}

fn ids_of(s: &LogicalSnapshot) -> Vec<&str> {
    s.records.iter().map(|r| r.message.id.as_str()).collect()
}

fn status_is(s: &LogicalSnapshot, code: crate::mailbox_logical::SourceErrorCode) -> bool {
    s.sources
        .iter()
        .any(|x| x.status == crate::mailbox_logical::SourceStatus::Error(code))
}

// Advisor ruling (STORY-1843 D1 follow-ups): a present, non-prunable
// worktree inventory cannot resolve left its `.deliveries/` unread, so the
// F2 gate fails closed. Here its `.git` pointer is garbage, so
// `common_git_of` fails while git still lists it, not prunable.
// trace:STORY-1843 | ai:claude
#[test]
fn story1843_d1_an_unresolvable_worktree_fails_closed() {
    let r = repo();
    let sidecar = DeliverySidecar::cohort("cohort", "reviewer", 20, &[uuid(1)]);
    send(&r.sib, "cohort", 20, Some(&sidecar));
    crate::mailbox::write_local_message(&r.main, &role_message("cohort", 20)).unwrap();
    send(&r.main, "legacy", 10, None);
    seed_direct_and_vendor(&r.main);
    let uuidless = reviewer("rev-old", None);
    let gitfile = r.sib.join(".git");
    let saved = std::fs::read(&gitfile).unwrap();
    std::fs::write(&gitfile, b"not a gitdir pointer\n").unwrap();
    let (_, s) = snap(&r.main, &uuidless);
    std::fs::write(&gitfile, saved).unwrap();
    assert!(
        status_is(&s, crate::mailbox_logical::SourceErrorCode::NotAGitCheckout),
        "{:?}",
        s.sources
    );
    assert!(!s.complete());
    assert_eq!(ids_of(&s), ["direct", "vendor"], "role mail fails closed");
    assert_eq!(visible(&r.main, &uuidless), ["legacy", "direct", "vendor"]);
}

// Advisor ruling: a worktree whose path cannot be resolved for a reason
// other than NotFound (here EACCES on its parent) and that git does not
// report prunable (it is locked; git reports an inaccessible worktree as
// prunable otherwise) is unreadable, not missing, so the gate fails closed.
// trace:STORY-1843 | ai:claude
#[test]
fn story1843_d1_an_inaccessible_worktree_fails_closed() {
    use std::os::unix::fs::PermissionsExt;
    let r = repo();
    let fence = r.main.parent().unwrap().join("fence");
    std::fs::create_dir_all(&fence).unwrap();
    let hidden = fence.join("hidden");
    git(
        &r.main,
        &["worktree", "add", "-q", "-b", "h", hidden.to_str().unwrap()],
    );
    let sidecar = DeliverySidecar::cohort("cohort", "reviewer", 20, &[uuid(1)]);
    send(&hidden, "cohort", 20, Some(&sidecar));
    crate::mailbox::write_local_message(&r.main, &role_message("cohort", 20)).unwrap();
    send(&r.main, "legacy", 10, None);
    seed_direct_and_vendor(&r.main);
    git(&r.main, &["worktree", "lock", hidden.to_str().unwrap()]);
    let uuidless = reviewer("rev-old", None);
    std::fs::set_permissions(&fence, std::fs::Permissions::from_mode(0o000)).unwrap();
    let denied = matches!(
        std::fs::canonicalize(&hidden),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied
    );
    let s = denied.then(|| snap(&r.main, &uuidless).1);
    std::fs::set_permissions(&fence, std::fs::Permissions::from_mode(0o755)).unwrap();
    let Some(s) = s else {
        eprintln!("SKIP: permissions are not enforced here (running as root?)");
        return;
    };
    assert!(
        status_is(&s, crate::mailbox_logical::SourceErrorCode::Unreadable),
        "{:?}",
        s.sources
    );
    assert!(!s.complete());
    assert_eq!(ids_of(&s), ["direct", "vendor"], "role mail fails closed");
    assert_eq!(visible(&r.main, &uuidless), ["legacy", "direct", "vendor"]);
}

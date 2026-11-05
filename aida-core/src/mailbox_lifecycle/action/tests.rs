//! trace:TASK-3074 | ai:codex
use super::*;
use crate::mailbox::{write_local_message, Recipient};
use crate::mailbox_lifecycle::{
    tests::{edit_record, message, repo},
    SourceState,
};
use crate::mailbox_logical::{logical_snapshot, MailboxContext, Query, ReadEvidence, ReadLimits};

fn audience(name: &str, role: bool) -> ResolvedAudience {
    if role {
        use crate::mailbox_identity::{
            IdentityEnv, MailIdentityFacts, PeopleCatalogView, SessionBinding, SessionBindingFacts,
        };
        return ResolvedAudience::from_facts(&MailIdentityFacts::observed(
            SessionBinding::Bound(SessionBindingFacts::fixture(name, "codex")),
            &PeopleCatalogView::default(),
            IdentityEnv::fixture(name, &[], Some("advisor")),
        ));
    }
    ResolvedAudience::from_identity_policy(vec![name.into()], vec![], vec![]).unwrap()
}
fn request() -> Message {
    Message {
        intent: Intent::Request,
        ..message("request-1")
    }
}
fn snapshot(root: &Path, who: &ResolvedAudience) -> (PathBuf, LogicalSnapshot) {
    let ctx = MailboxContext::resolve(root, None).unwrap();
    let snap = logical_snapshot(&ctx, who, &Query::Addressable, &ReadLimits::explicit_read());
    (ctx.repository().unwrap().path().to_path_buf(), snap)
}
fn prepare(root: &Path, who: &ResolvedAudience, outcome: Outcome) -> Result<ActionPlan> {
    let (common, snap) = snapshot(root, who);
    ActionPlan::prepare(
        &snap,
        &common,
        who,
        snap.records.first().context("no mail")?,
        outcome,
        "took responsibility",
        Some("PR-123"),
        None,
    )
}
fn path(common: &Path) -> PathBuf {
    common.join("aida/mail-lifecycle").join(FILE)
}

#[test]
fn revisions_win_without_marking_read_or_mutating_message() {
    let dir = repo();
    let root = dir.path();
    let who = audience("bob", false);
    let msg = request();
    write_local_message(root, &msg).unwrap();
    let (common, _) = snapshot(root, &who);
    assert!(latest(&common, &msg).unwrap().is_none());
    assert!(!path(&common).exists());
    prepare(root, &who, Outcome::Acted)
        .unwrap()
        .commit(|| Ok(who.clone()))
        .unwrap();
    prepare(root, &who, Outcome::Declined)
        .unwrap()
        .commit(|| Ok(who.clone()))
        .unwrap();
    assert_eq!(
        latest(&common, &msg).unwrap().unwrap().outcome,
        Outcome::Declined
    );
    assert_eq!(
        read_ledger(path(&common).parent().unwrap())
            .unwrap()
            .revisions
            .len(),
        2
    );
    let (_, snap) = snapshot(root, &who);
    assert_eq!(snap.records[0].read, ReadEvidence::Unread);
    assert_eq!(snap.records[0].message.body, msg.body);
    assert!(!common.join("aida/mail-lifecycle/receipts").exists());
}

#[test]
fn read_ack_legacy_read_reply_and_wake_do_not_supply_action() {
    let dir = repo();
    let root = dir.path();
    let who = audience("bob", false);
    let msg = request();
    write_local_message(root, &msg).unwrap();
    let (common, snap) = snapshot(root, &who);
    let ack = ack::AckPlan::prepare(&snap, Some(&common), &who, &[&snap.records[0]])
        .unwrap()
        .unwrap();
    ack.commit(&who).unwrap();
    // A reply is simply mail; no wake or ACK writer touches the action ledger.
    write_local_message(
        root,
        &Message {
            id: "reply".into(),
            in_reply_to: Some(msg.id.clone()),
            ..message("reply")
        },
    )
    .unwrap();
    assert!(latest(&common, &msg).unwrap().is_none());
    // Already ACKed requests can still receive an explicit action receipt.
    let (_, snap) = snapshot(root, &who);
    let r = snap
        .records
        .iter()
        .find(|r| r.message.id == msg.id)
        .unwrap();
    ActionPlan::prepare(
        &snap,
        &common,
        &who,
        r,
        Outcome::Acted,
        "handled",
        Some("PR-123"),
        None,
    )
    .unwrap()
    .commit(|| Ok(who.clone()))
    .unwrap();
    let ledger_before = std::fs::read(path(&common)).unwrap();
    let marks = root.join(".aida/mailbox/.read");
    std::fs::create_dir_all(&marks).unwrap();
    std::fs::write(marks.join("bob.txt"), "100").unwrap();
    assert_eq!(std::fs::read(path(&common)).unwrap(), ledger_before);
}

#[test]
fn sender_and_foreign_recipient_cannot_act_but_one_shared_reader_can() {
    let dir = repo();
    let root = dir.path();
    write_local_message(root, &request()).unwrap();
    assert!(prepare(root, &audience("alice", false), Outcome::Acted).is_err());
    assert!(prepare(root, &audience("carol", false), Outcome::Acted).is_err());
    let shared = Message {
        to: Recipient::Agent("advisor".into()),
        ..request()
    };
    write_local_message(root, &shared).unwrap();
    let hinted = ResolvedAudience::from_identity_policy(
        vec!["operator".into()],
        vec![],
        vec!["advisor".into()],
    )
    .unwrap();
    assert!(prepare(root, &hinted, Outcome::Declined)
        .err()
        .unwrap()
        .to_string()
        .contains("role hint"));
    let who = audience("advisor-1", true);
    let receipt = prepare(root, &who, Outcome::Declined)
        .unwrap()
        .commit(|| Ok(who.clone()))
        .unwrap();
    assert_eq!(receipt.actor, "advisor-1");
    assert_eq!(receipt.keys, vec!["advisor"]);
    let (common, _) = snapshot(root, &who);
    assert_eq!(latest(&common, &shared).unwrap().unwrap(), receipt);
}

#[test]
fn changed_authority_is_revalidated_under_both_locks() {
    let dir = repo();
    let root = dir.path();
    let who = audience("bob", false);
    write_local_message(root, &request()).unwrap();
    let plan = prepare(root, &who, Outcome::Acted).unwrap();
    let (result, classes) = crate::mailbox_lifecycle::observe_lock_classes(|| {
        plan.commit(|| {
            assert!(crate::mailbox_lifecycle::lifecycle_lock_held());
            Ok(audience("carol", false))
        })
    });
    assert_eq!(classes, vec!["lifecycle", "receipt"]);
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("authority changed"));
    let (common, _) = snapshot(root, &who);
    assert!(!path(&common).exists());
}

#[test]
fn deferred_deadlines_and_acted_evidence_are_explicit_and_bounded() {
    let now = 100;
    assert!(validate_input(Outcome::Acted, "handled", None, None, now).is_err());
    assert!(validate_input(Outcome::Declined, "", None, None, now).is_err());
    assert!(validate_input(Outcome::Declined, "no", None, Some(101), now).is_err());
    for until in [None, Some(now), Some(now - 1), Some(now + DAY_MS + 1)] {
        assert!(validate_input(Outcome::Deferred, "later", None, until, now).is_err());
    }
    assert!(validate_input(Outcome::Deferred, "later", None, Some(now + DAY_MS), now).is_ok());
    assert!(validate_input(Outcome::Acted, "handled", Some("   "), None, now).is_err());
    assert!(validate_input(
        Outcome::Declined,
        &"x".repeat(MAX_TEXT + 1),
        None,
        None,
        now
    )
    .is_err());
}

#[test]
fn changed_replica_and_retiring_source_refuse_without_ledger_write() {
    let dir = repo();
    let root = dir.path();
    let who = audience("bob", false);
    write_local_message(root, &request()).unwrap();
    let plan = prepare(root, &who, Outcome::Acted).unwrap();
    write_local_message(
        root,
        &Message {
            body: "changed".into(),
            ..request()
        },
    )
    .unwrap();
    assert!(plan.commit(|| Ok(who.clone())).is_err());
    let plan = prepare(root, &who, Outcome::Acted).unwrap();
    edit_record(root, |r| {
        r.state = SourceState::Fenced {
            op_id: "op".into(),
            reason: "test".into(),
        }
    });
    assert!(plan.commit(|| Ok(who.clone())).is_err());
    let (common, _) = snapshot(root, &who);
    assert!(!path(&common).exists());
}

#[test]
fn archive_preserves_action_while_withdrawal_and_nonrequests_refuse_new_action() {
    let dir = repo();
    let root = dir.path();
    let who = audience("bob", false);
    let msg = request();
    write_local_message(root, &msg).unwrap();
    prepare(root, &who, Outcome::Acted)
        .unwrap()
        .commit(|| Ok(who.clone()))
        .unwrap();
    let (common, _) = snapshot(root, &who);
    let archived = Message {
        archived: true,
        ..msg.clone()
    };
    write_local_message(root, &archived).unwrap();
    assert!(latest(&common, &archived).unwrap().is_some());
    prepare(root, &who, Outcome::Declined)
        .unwrap()
        .commit(|| Ok(who.clone()))
        .unwrap();
    for changed in [
        Message {
            retracted: true,
            ..msg.clone()
        },
        Message {
            deleted: true,
            ..msg.clone()
        },
        Message {
            intent: Intent::Fyi,
            ..msg.clone()
        },
    ] {
        write_local_message(root, &changed).unwrap();
        assert!(prepare(root, &who, Outcome::Acted).is_err());
    }
}

#[test]
fn corrupt_unsupported_and_unsafe_ledgers_fail_closed() {
    let dir = repo();
    let root = dir.path();
    let who = audience("bob", false);
    write_local_message(root, &request()).unwrap();
    let (common, _) = snapshot(root, &who);
    let anchor = ensure_anchor(&common).unwrap();
    for bytes in [b"{bad".as_slice(), b"{\"schema\":2,\"revisions\":[]}"] {
        write_durable(&anchor.join(FILE), bytes).unwrap();
        assert!(prepare(root, &who, Outcome::Acted)
            .unwrap()
            .commit(|| Ok(who.clone()))
            .is_err());
        assert_eq!(std::fs::read(anchor.join(FILE)).unwrap(), bytes);
    }
    std::fs::remove_file(anchor.join(FILE)).unwrap();
    let target = root.join("outside");
    std::fs::write(&target, "untouched").unwrap();
    std::os::unix::fs::symlink(&target, anchor.join(FILE)).unwrap();
    assert!(latest(&common, &request()).is_err());
    assert!(prepare(root, &who, Outcome::Acted)
        .unwrap()
        .commit(|| Ok(who.clone()))
        .is_err());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "untouched");
}

#[test]
fn full_ledger_refuses_without_pruning() {
    let dir = repo();
    let root = dir.path();
    let who = audience("bob", false);
    write_local_message(root, &request()).unwrap();
    let receipt = prepare(root, &who, Outcome::Acted)
        .unwrap()
        .commit(|| Ok(who.clone()))
        .unwrap();
    let (common, _) = snapshot(root, &who);
    let anchor = path(&common).parent().unwrap().to_path_buf();
    let full = serde_json::to_vec(&Ledger {
        schema: 1,
        revisions: vec![receipt; MAX_REVISIONS],
    })
    .unwrap();
    write_durable(&anchor.join(FILE), &full).unwrap();
    assert!(prepare(root, &who, Outcome::Declined)
        .unwrap()
        .commit(|| Ok(who.clone()))
        .unwrap_err()
        .to_string()
        .contains("full"));
    assert_eq!(std::fs::read(path(&common)).unwrap(), full);
}

#[test]
fn partial_snapshot_refuses_even_when_the_request_is_visible() {
    let dir = repo();
    let foreign = repo();
    let who = audience("bob", false);
    write_local_message(dir.path(), &request()).unwrap();
    let ctx = MailboxContext::resolve(dir.path(), Some(foreign.path())).unwrap();
    let snap = logical_snapshot(&ctx, &who, &Query::Inbox, &ReadLimits::explicit_read());
    assert!(!snap.complete());
    assert!(ActionPlan::prepare(
        &snap,
        ctx.repository().unwrap().path(),
        &who,
        &snap.records[0],
        Outcome::Acted,
        "handled",
        Some("PR-1"),
        None
    )
    .is_err());
}

#[test]
fn plans_prepared_together_retain_both_revisions() {
    let dir = repo();
    let root = dir.path();
    let who = audience("bob", false);
    write_local_message(root, &request()).unwrap();
    let first = prepare(root, &who, Outcome::Acted).unwrap();
    let second = prepare(root, &who, Outcome::Declined).unwrap();
    first.commit(|| Ok(who.clone())).unwrap();
    second.commit(|| Ok(who.clone())).unwrap();
    let (common, _) = snapshot(root, &who);
    let ledger = read_ledger(path(&common).parent().unwrap()).unwrap();
    assert_eq!(ledger.revisions.len(), 2);
    assert_eq!(ledger.revisions[0].outcome, Outcome::Acted);
    assert_eq!(ledger.revisions[1].outcome, Outcome::Declined);
}

#[test]
fn conflicting_copies_prevent_disposition_of_other_visible_requests() {
    let dir = repo();
    let root = dir.path();
    let who = audience("bob", false);
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
    write_local_message(root, &request()).unwrap();
    write_local_message(
        &sib,
        &Message {
            body: "diverged".into(),
            ..request()
        },
    )
    .unwrap();
    write_local_message(
        root,
        &Message {
            id: "other".into(),
            ..request()
        },
    )
    .unwrap();
    let (common, snap) = snapshot(root, &who);
    assert!(!snap.ack_eligible());
    let record = snap
        .records
        .iter()
        .find(|r| r.message.id == "other")
        .unwrap();
    assert!(ActionPlan::prepare(
        &snap,
        &common,
        &who,
        record,
        Outcome::Acted,
        "handled",
        Some("PR-1"),
        None
    )
    .is_err());
    assert!(!path(&common).exists());
}

#[test]
fn passive_projection_and_writer_refuse_symlinked_anchor() {
    let dir = repo();
    let root = dir.path();
    let who = audience("bob", false);
    write_local_message(root, &request()).unwrap();
    let plan = prepare(root, &who, Outcome::Acted).unwrap();
    let (common, _) = snapshot(root, &who);
    let anchor = common.join("aida/mail-lifecycle");
    std::fs::remove_dir_all(&anchor).unwrap();
    let outside = root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, &anchor).unwrap();
    assert!(latest(&common, &request()).is_err());
    assert!(plan.commit(|| Ok(who.clone())).is_err());
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn deferred_commit_preserves_deadline_and_server_timestamp() {
    let dir = repo();
    let root = dir.path();
    let who = audience("bob", false);
    write_local_message(root, &request()).unwrap();
    let (common, snap) = snapshot(root, &who);
    let now = chrono::Utc::now().timestamp_millis();
    let until = now + 60_000;
    let plan = ActionPlan::prepare(
        &snap,
        &common,
        &who,
        &snap.records[0],
        Outcome::Deferred,
        "waiting",
        None,
        Some(until),
    )
    .unwrap();
    let receipt = plan.commit(|| Ok(who.clone())).unwrap();
    assert_eq!(receipt.until, Some(until));
    assert!(receipt.recorded_at >= now);
    assert_eq!(latest(&common, &request()).unwrap(), Some(receipt));
}

#[test]
fn ended_session_binding_cannot_commit_prepared_response() {
    use crate::mailbox_identity::{
        IdentityEnv, MailIdentityFacts, PeopleCatalogView, SessionBinding, SessionBindingFacts,
    };
    let dir = repo();
    let root = dir.path();
    let row = root.join("seat.toml");
    let pid = std::process::id();
    let start = crate::liveness::process_start_identity(pid).unwrap();
    let body = format!("id = \"seat\"\nagent_type = \"codex\"\npid = {pid}\npid_start = \"{start}\"\nname = \"seat-a\"\n");
    std::fs::write(&row, &body).unwrap();
    let env = IdentityEnv::fixture("operator", &[], None);
    let binding = SessionBindingFacts::verify(&row, &env).unwrap();
    let who = ResolvedAudience::from_facts(&MailIdentityFacts::observed(
        SessionBinding::Bound(binding),
        &PeopleCatalogView::default(),
        env.clone(),
    ));
    write_local_message(
        root,
        &Message {
            to: Recipient::Agent("seat-a".into()),
            ..request()
        },
    )
    .unwrap();
    let plan = prepare(root, &who, Outcome::Acted).unwrap();
    std::fs::write(&row, format!("{body}ended_at = \"2026-11-05T20:00:00Z\"\n")).unwrap();
    let result = plan.commit(|| {
        let binding = SessionBindingFacts::verify(&row, &env)
            .map_err(|why| anyhow::anyhow!("session authority refused: {why:?}"))?;
        Ok(ResolvedAudience::from_facts(&MailIdentityFacts::observed(
            SessionBinding::Bound(binding),
            &PeopleCatalogView::default(),
            env,
        )))
    });
    assert!(result.unwrap_err().to_string().contains("Ended"));
    let (common, _) = snapshot(root, &who);
    assert!(!path(&common).exists());
}

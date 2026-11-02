//! Production guard and supported own inbox, on the same isolated repository.
// trace:BUG-b6b1278890774ef993d2ebc327862470-178 | ai:codex
use super::*;
use crate::mailbox_view::tests::{mail, mail_repo, put_canonical, MailRepo};
use crate::mailbox_view::{self as view, explicit_limits, own_audience_in, ReadState};
use aida_core::mailbox_delivery::{
    binding_file_name, handover_record, write_handover, DeliverySidecar, RecipientUuid,
};
use aida_core::mailbox_logical::MailboxContext;
use std::process::{Child, Command};

const SESSION: &str = "bug2405-synthetic-own-session";
fn isolated() -> crate::test_env::AgentSessionIsolation {
    crate::test_env::AgentSessionIsolation::apply(&[
        ("CODEX_THREAD_ID", Some(SESSION)),
        ("AIDA_AGENT_TYPE", Some("codex")),
        ("AIDA_SESSION_ROLE", Some("reviewer")),
        ("AIDA_USER", Some("fixture")),
    ])
}
fn reader(r: &MailRepo, pid: u32, vendor: &str, name: &str) -> (AgentRegistryEntry, RecipientUuid) {
    let mut e = crate::agent_registry::register_existing_agent(
        &r.main,
        vendor,
        pid,
        "reviewer".into(),
        None,
        r.main.clone(),
        Some(name.into()),
        None,
    )
    .unwrap();
    if pid == std::process::id() {
        e.native_session_id = Some(SESSION.into());
    }
    e.mail_wake = (vendor == "codex").then_some(MailWake::Armed);
    crate::agent_registry::write_entry(&r.main, &e).unwrap();
    crate::mailbox_resolver::reserve_test_reader(&r.main, &e);
    let uuid = aida_core::mailbox_identity::reservation_recipient(
        &crate::agent_registry::agents_dir(&r.main),
        name,
        &e.id,
        &[],
    )
    .unwrap();
    (e, uuid)
}
struct FixtureProcess(Child);
impl FixtureProcess {
    fn new() -> Self {
        Self(Command::new("sleep").arg("300").spawn().unwrap())
    }
    fn pid(&self) -> u32 {
        self.0.id()
    }
}
impl Drop for FixtureProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn send(root: &Path, id: &str, owners: Option<&[RecipientUuid]>) -> DeliverySidecar {
    let ts = (Utc::now() - Duration::hours(2)).timestamp_millis();
    let mut m = mail(id, "reviewer", ts, false);
    m.intent = Intent::Request;
    let sidecar = owners
        .map(|o| DeliverySidecar::cohort(id, "reviewer", ts, o))
        .unwrap_or_else(|| DeliverySidecar::queued(id, "reviewer", ts));
    crate::mailbox_store::write_message_with_deliveries(root, &m, Some(&sidecar)).unwrap();
    sidecar
}
fn own(r: &MailRepo) -> view::OwnMail {
    view::read_own_mail(
        &r.main,
        &r.store,
        &own_audience_in(&r.main),
        &explicit_limits(),
    )
}
fn unread(r: &MailRepo, id: &str) -> bool {
    own(r).unread().iter().any(|rec| rec.id == id)
}
fn found(r: &MailRepo, id: &str) -> bool {
    scan(&r.main)
        .iter()
        .any(|f| f.id == format!("mail-backlog:reviewer:{id}"))
}
fn incomplete(r: &MailRepo) -> bool {
    scan(&r.main).iter().any(|f| f.id == "mail-backlog:unknown")
}
fn inbox(r: &MailRepo, peek: bool) {
    crate::mailbox_cmd::handle_mailbox_command(
        &crate::cli::MailboxCommand::Inbox {
            agent: None,
            all: false,
            archived: false,
            peek,
            unread: false,
            recent_read_tail: 0,
        },
        &r.store,
    )
    .unwrap();
}
fn common(r: &MailRepo) -> PathBuf {
    MailboxContext::resolve(&r.main, Some(&r.store))
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

#[test]
fn old_cohort_new_owner_uuid_receipts_and_real_postflush_ack() {
    let _env = isolated();
    let r = mail_repo();
    let (_, b) = reader(&r, std::process::id(), "codex", "reader-b");
    let child = FixtureProcess::new();
    let (mut c_row, c) = reader(&r, child.pid(), "codex", "reader-c");
    let old_process = FixtureProcess::new();
    let (mut old_row, retired) = reader(&r, old_process.pid(), "codex", "reader-a");
    old_row.ended_at = Some(Utc::now());
    crate::agent_registry::write_entry(&r.main, &old_row).unwrap();
    send(&r.sib, "old", Some(&[retired]));
    send(&r.sib, "mine", Some(&[b.clone()]));
    send(&r.sib, "both", Some(&[b.clone(), c]));
    assert!(!unread(&r, "old"));
    assert!(!found(&r, "old"));
    assert!(unread(&r, "mine"));
    assert!(found(&r, "mine"));
    crate::mailbox_store::set_watermark(&r.main, "reviewer", Utc::now().timestamp_millis())
        .unwrap();
    crate::mailbox_store::record_seen(&r.sib, "reviewer", &["mine", "both"]).unwrap();
    assert!(unread(&r, "mine"));
    assert!(found(&r, "mine"));
    inbox(&r, true);
    assert!(found(&r, "mine"));
    inbox(&r, false); // actual supported output, flush and ACK
    assert!(!unread(&r, "mine"));
    assert!(!found(&r, "mine"));
    assert!(crate::mailbox_store::read_receipts(&r.main, &b.receipt_key()).contains_key("mine"));
    assert!(found(&r, "both"), "C's unread ownership survives B's ACK");
    c_row.ended_at = Some(Utc::now());
    crate::agent_registry::write_entry(&r.main, &c_row).unwrap();
    std::fs::remove_dir_all(r.main.join(".aida/mailbox/.read-receipts")).unwrap();
    assert!(
        !found(&r, "mine"),
        "common-Git anchor survives local receipt removal"
    );
    assert!(!incomplete(&r));
    send(&r.sib, "sibling-ack", Some(&[b.clone()]));
    let audience = own_audience_in(&r.sib);
    let v = view::read_own_mail(&r.sib, &r.store, &audience, &explicit_limits());
    let displayed: Vec<_> = v
        .records()
        .iter()
        .filter(|r| r.message.id == "sibling-ack")
        .collect();
    let plan = view::plan_ack(&v, &displayed).unwrap().unwrap();
    let mut output = Vec::new();
    use std::io::Write;
    writeln!(&mut output, "{}", displayed[0].message.body).unwrap();
    output.flush().unwrap();
    view::commit_ack(&r.sib, plan, &own_audience_in(&r.sib)).unwrap();
    assert!(
        crate::mailbox_store::read_receipts(&r.sib, &b.receipt_key()).contains_key("sibling-ack")
    );
    assert!(!found(&r, "sibling-ack"));
}

#[test]
fn ownership_filtered_before_wake_preference_and_reminder_ack_is_separate() {
    let _env = isolated();
    let r = mail_repo();
    let (_, b) = reader(&r, std::process::id(), "codex", "reader-b");
    let child = FixtureProcess::new();
    let (_, c) = reader(&r, child.pid(), "claude", "reader-c");
    send(&r.main, "codex-owned", Some(&[b.clone()]));
    let now = Utc::now();
    let policy = Policy::default();
    let (items, unknown) = inspect_backlog(&r.main, &r.store, &policy, now, &explicit_limits());
    assert!(!unknown);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].wake, Wake::Mail("reader-b".into()));
    scan_and_route(&r.main, true);
    let before = load_nudged(&r.main);
    assert_eq!(before.len(), 1);
    let v = own(&r);
    let reminders: Vec<_> = v
        .records()
        .iter()
        .filter(|rec| rec.message.from == REMINDER_SENDER)
        .collect();
    assert_eq!(reminders.len(), 1);
    // Use normal explicit ACK plan, model delivered output and flush, then
    // current-own identity revalidation. Only the reminder was displayed.
    let plan = view::plan_ack(&v, &reminders).unwrap().unwrap();
    let mut output = Vec::new();
    use std::io::Write;
    writeln!(&mut output, "{}", reminders[0].message.body).unwrap();
    output.flush().unwrap();
    view::commit_ack(&r.main, plan, &own_audience_in(&r.main)).unwrap();
    assert!(unread(&r, "codex-owned"));
    assert!(found(&r, "codex-owned"));
    scan_and_route(&r.main, true);
    assert_eq!(
        load_nudged(&r.main),
        before,
        "landed nudge respects re-nudge threshold"
    );
    send(&r.main, "claude-owned", Some(&[b, c]));
    let (items, _) = inspect_backlog(&r.main, &r.store, &policy, Utc::now(), &explicit_limits());
    assert_eq!(
        items
            .iter()
            .find(|i| i.id.ends_with(":claude-owned"))
            .unwrap()
            .wake,
        Wake::Seat("reviewer".into())
    );
    scan_and_route(&r.main, true);
    crate::schedule_ledger::write_cas(&r.store, &route_job_name("reviewer"), |ledger| {
        ledger.due_since = None;
    })
    .unwrap();
    assert!(found(&r, "claude-owned"));
    assert!(unread(&r, "claude-owned"));
    let mut e = crate::agent_registry::load_entries(&r.main)
        .into_iter()
        .find(|(_, e)| e.name.as_deref() == Some("reader-b"))
        .unwrap()
        .1;
    e.mail_wake = None;
    crate::agent_registry::write_entry(&r.main, &e).unwrap();
    let (items, _) = inspect_backlog(&r.main, &r.store, &policy, Utc::now(), &explicit_limits());
    assert_eq!(
        items
            .iter()
            .find(|i| i.id.ends_with(":codex-owned"))
            .unwrap()
            .wake,
        Wake::Operator
    );
}

#[test]
fn queued_pending_real_bind_handover_and_retained_sources() {
    let _env = isolated();
    let r = mail_repo();
    let (_, b) = reader(&r, std::process::id(), "codex", "reader-b");
    let child = FixtureProcess::new();
    let (_, c) = reader(&r, child.pid(), "codex", "reader-c");
    let queued = send(&r.sib, "queued", None);
    assert!(!unread(&r, "queued"));
    assert!(found(&r, "queued"));
    let bound = common(&r).join("aida/mail-lifecycle/deliveries/bound");
    assert!(
        !bound
            .join(binding_file_name(&queued.deliveries[0].delivery_id))
            .exists(),
        "guard does not bind"
    );
    let v = view::read_own_mail_binding(
        &r.main,
        &r.store,
        &own_audience_in(&r.main),
        &explicit_limits(),
        view::BindWait::Explicit,
    );
    assert!(v.unread().iter().any(|rec| rec.id == "queued"));
    assert!(found(&r, "queued"));
    let ctx = MailboxContext::resolve(&r.main, Some(&r.store)).unwrap();
    assert!(
        matches!(aida_core::mailbox_delivery::bind_queued_delivery(ctx.repository().map(|r|r.path()),&r.main,
        &queued.deliveries[0].delivery_id,&c,||true,std::time::Duration::from_secs(1)),
        Ok(aida_core::mailbox_delivery::BindOutcome::AlreadyBound(owner)) if owner == b)
    );
    let old = RecipientUuid::parse(&uuid::Uuid::new_v4().to_string()).unwrap();
    let sidecar = send(&r.main, "handed", Some(&[old.clone()]));
    send(&r.main, "invalid", Some(&[old]));
    write_handover(
        &r.sib,
        &handover_record("handed", &sidecar.deliveries[0].delivery_id, &b),
    )
    .unwrap();
    write_handover(&r.sib, &handover_record("invalid", "unknown-delivery", &b)).unwrap();
    assert!(unread(&r, "handed"));
    assert!(found(&r, "handed"));
    assert!(!unread(&r, "invalid"));
    assert!(!found(&r, "invalid"));
    // A surviving canonical replica must still use its retired sidecar.
    let sidecar = send(
        &r.sib,
        "retired",
        Some(&[RecipientUuid::parse(&uuid::Uuid::new_v4().to_string()).unwrap()]),
    );
    let m = crate::mailbox_store::read_local_messages(&r.sib)
        .unwrap()
        .into_iter()
        .find(|m| m.id == "retired")
        .unwrap();
    put_canonical(&r.store, &m);
    let retained = common(&r).join("aida/mail-lifecycle/retained/k1/1/data/.deliveries");
    private_dir(&retained);
    std::fs::write(
        retained.join("retired.json"),
        serde_json::to_vec(&sidecar).unwrap(),
    )
    .unwrap();
    std::fs::remove_file(r.sib.join(".aida/mailbox/.deliveries/retired.json")).unwrap();
    std::fs::remove_file(r.sib.join(".aida/mailbox/retired.json")).unwrap();
    std::fs::write(retained.join("unrelated.json"), "broken").unwrap();
    assert!(!found(&r, "retired"));
    assert!(!unread(&r, "retired"));
    assert!(!incomplete(&r));
}

#[test]
fn local_unknown_global_partial_preserve_findings_and_nudge_state() {
    let _env = isolated();
    let r = mail_repo();
    let (_, b) = reader(&r, std::process::id(), "codex", "reader-b");
    send(&r.main, "known", Some(&[b]));
    let torn = send(&r.main, "torn", None);
    scan_and_route(&r.main, true);
    let previous = load_nudged(&r.main);
    assert!(previous.contains_key("reviewer:torn"));
    let bound = common(&r).join("aida/mail-lifecycle/deliveries/bound");
    private_dir(&bound);
    std::fs::write(
        bound.join(binding_file_name(&torn.deliveries[0].delivery_id)),
        "",
    )
    .unwrap();
    assert!(incomplete(&r));
    assert!(found(&r, "known"));
    assert!(!found(&r, "torn"));
    assert!(
        !own(&r).is_partial(),
        "own complete compatibility remains unchanged"
    );
    scan_and_route(&r.main, true);
    assert_eq!(load_nudged(&r.main), previous);
    let retained = common(&r).join("aida/mail-lifecycle/retained/k1/1/data/.deliveries");
    private_dir(&retained);
    let ts = (Utc::now() - Duration::hours(2)).timestamp_millis();
    let mut legacy = mail("retained-corrupt", "reviewer", ts, false);
    legacy.intent = Intent::Request;
    crate::mailbox_store::write_message(&r.main, &legacy).unwrap();
    std::fs::write(retained.join("retained-corrupt.json"), "broken").unwrap();
    assert!(incomplete(&r));
    assert!(found(&r, "known"));
    assert!(!found(&r, "retained-corrupt"));
    assert!(!own(&r).is_partial());
    std::fs::create_dir_all(r.sib.join(".aida/mailbox")).unwrap();
    std::fs::write(r.sib.join(".aida/mailbox/broken.json"), "broken").unwrap();
    assert!(incomplete(&r));
    assert!(found(&r, "known"));
    scan_and_route(&r.main, true);
    assert_eq!(load_nudged(&r.main), previous);
    assert!(!!own(&r).is_partial());
    let mut limits = explicit_limits();
    limits.max_entries = 0;
    let (_, unknown) = inspect_backlog(&r.main, &r.store, &Policy::default(), Utc::now(), &limits);
    assert!(unknown);
}

#[test]
fn legacy_watermark_is_source_scoped_and_receipt_failure_is_unknown() {
    let _env = isolated();
    let r = mail_repo();
    let (_, b) = reader(&r, std::process::id(), "codex", "reader-b");
    let ts = (Utc::now() - Duration::hours(2)).timestamp_millis();
    let mut m = mail("legacy", "reviewer", ts, false);
    m.intent = Intent::Request;
    crate::mailbox_store::write_message(&r.sib, &m).unwrap();
    crate::mailbox_store::set_watermark(&r.main, "reviewer", Utc::now().timestamp_millis())
        .unwrap();
    assert!(unread(&r, "legacy"));
    assert!(found(&r, "legacy"));
    crate::mailbox_store::set_watermark(&r.sib, "reviewer", Utc::now().timestamp_millis()).unwrap();
    assert!(!found(&r, "legacy"));
    assert_eq!(
        ReadState::of(
            own(&r)
                .records()
                .iter()
                .find(|r| r.message.id == "legacy")
                .unwrap()
        ),
        ReadState::ReadLegacy
    );
    send(&r.main, "bad-receipt", Some(&[b.clone()]));
    send(&r.main, "good", Some(&[b.clone()]));
    let receipts = r
        .sib
        .join(".aida/mailbox/.read-receipts")
        .join(crate::mailbox_store::sanitize_id(&b.receipt_key()));
    std::fs::create_dir_all(&receipts).unwrap();
    std::fs::write(receipts.join("bad-receipt.txt"), "broken").unwrap();
    assert!(incomplete(&r));
    assert!(!found(&r, "bad-receipt"));
    assert!(found(&r, "good"));
}

#[test]
fn unverified_reader_and_anchor_errors_never_become_all_clear() {
    let _env = isolated();
    let r = mail_repo();
    let (mut row, b) = reader(&r, std::process::id(), "codex", "reader-b");
    send(&r.main, "mine", Some(&[b.clone()]));
    scan_and_route(&r.main, true);
    let before = load_nudged(&r.main);
    let reservation = crate::agent_registry::agents_dir(&r.main).join("handles/reader-b.toml");
    let saved = std::fs::read(&reservation).unwrap();
    std::fs::remove_file(&reservation).unwrap();
    assert!(incomplete(&r));
    assert!(!found(&r, "mine"));
    scan_and_route(&r.main, true);
    assert_eq!(load_nudged(&r.main), before);
    std::fs::write(&reservation, saved).unwrap();
    let start = row.pid_start.take();
    crate::agent_registry::write_entry(&r.main, &row).unwrap();
    assert!(incomplete(&r));
    scan_and_route(&r.main, true);
    assert_eq!(load_nudged(&r.main), before);
    row.pid_start = start;
    crate::agent_registry::write_entry(&r.main, &row).unwrap();
    let anchor = common(&r).join("aida/mail-lifecycle/receipts");
    private_dir(&anchor);
    std::fs::write(
        anchor.join(aida_core::mailbox_logical::receipt_ledger_name(
            &b.receipt_key(),
        )),
        "broken",
    )
    .unwrap();
    assert!(incomplete(&r));
    assert!(!found(&r, "mine"));
    assert!(own(&r).is_partial());
    scan_and_route(&r.main, true);
    assert_eq!(load_nudged(&r.main), before);
}

#[test]
fn queued_identity_loss_lock_refusal_and_notice_never_ack_target() {
    let _env = isolated();
    let r = mail_repo();
    reader(&r, std::process::id(), "codex", "reader-b");
    send(&r.main, "queued", None);
    let audience = own_audience_in(&r.main);
    let reservation = crate::agent_registry::agents_dir(&r.main).join("handles/reader-b.toml");
    let saved = std::fs::read(&reservation).unwrap();
    std::fs::remove_file(&reservation).unwrap();
    let v = view::read_own_mail_binding(
        &r.main,
        &r.store,
        &audience,
        &explicit_limits(),
        view::BindWait::Notice,
    );
    assert!(v.unread().is_empty());
    assert_eq!(v.pending_deliveries(), 1);
    std::fs::write(&reservation, saved).unwrap();
    // Hold the real lifecycle lock; notice's bounded bind must refuse.
    let anchor = common(&r).join("aida/mail-lifecycle");
    private_dir(&anchor);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(anchor.join("lifecycle.lock"))
        .unwrap();
    use std::os::fd::AsRawFd;
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let v = view::read_own_mail_binding(
        &r.main,
        &r.store,
        &audience,
        &explicit_limits(),
        view::BindWait::Notice,
    );
    assert!(v.unread().is_empty());
    assert_eq!(v.pending_deliveries(), 1);
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) }, 0);
    let v = view::read_own_mail_binding(
        &r.main,
        &r.store,
        &audience,
        &explicit_limits(),
        view::BindWait::Notice,
    );
    assert!(v.unread().iter().any(|m| m.id == "queued"));
    let _notice = crate::mailbox_cmd::notice_text(&v, &["reviewer".into()], 10);
    assert!(unread(&r, "queued"));
    assert!(found(&r, "queued"));
}

#[test]
fn production_age_dedup_and_renudge_boundaries() {
    let _env = isolated();
    let r = mail_repo();
    let (_, b) = reader(&r, std::process::id(), "codex", "reader-b");
    send(&r.main, "replica", Some(&[b]));
    let m = crate::mailbox_store::read_local_messages(&r.main)
        .unwrap()
        .into_iter()
        .find(|m| m.id == "replica")
        .unwrap();
    put_canonical(&r.store, &m);
    let now = Utc::now();
    let ts = (now - Duration::minutes(30)).timestamp_millis();
    let mut boundary = mail("boundary", "reviewer", ts, false);
    boundary.intent = Intent::Handoff;
    crate::mailbox_store::write_message(&r.main, &boundary).unwrap();
    let policy = Policy::default();
    let (items, unknown) = inspect_backlog(
        &r.main,
        &r.store,
        &policy,
        now - Duration::milliseconds(1),
        &explicit_limits(),
    );
    assert!(!unknown);
    assert_eq!(
        items.len(),
        1,
        "replicas dedup; below age boundary excluded"
    );
    let (items, unknown) = inspect_backlog(&r.main, &r.store, &policy, now, &explicit_limits());
    assert!(!unknown);
    assert_eq!(items.len(), 2, "exact age boundary included");
    let mut nudged = BTreeMap::new();
    nudged.insert("reviewer:replica".into(), now - Duration::minutes(60));
    nudged.insert(
        "reviewer:boundary".into(),
        now - Duration::minutes(60) + Duration::milliseconds(1),
    );
    let due = due_nudges(&items, &nudged, &policy, now);
    let ids: Vec<_> = due.values().flatten().map(|b| b.id.as_str()).collect();
    assert_eq!(ids, ["reviewer:replica"]);
}

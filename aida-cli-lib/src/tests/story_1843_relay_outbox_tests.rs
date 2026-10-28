//! Relay end notices and relay-label delivery records (STORY-1843 D2b).
// trace:STORY-1843 | ai:claude

use super::outbox::{self, DestinationKind, NoticeState};
use super::*;
use crate::mailbox_resolver::{delivery_sidecar, RecipientDirectory};
use aida_core::mailbox_delivery::{DeliverySidecar, RecipientUuid};
use fs2::FileExt;
use std::io::Write;

struct HeldLock {
    _file: std::fs::File,
    path: PathBuf,
}

fn held_lock(dir: &Path, name: &str, holder: u32) -> HeldLock {
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(file, "{holder}").unwrap();
    file.lock_exclusive().unwrap();
    HeldLock { _file: file, path }
}

fn me() -> u32 {
    std::process::id()
}

/// A live run: its id, its capability and the run lock its controller holds.
fn bind_live(project: &Path, label: &str) -> (String, String, HeldLock) {
    let lock = held_lock(project, &format!("{label}.lock"), me());
    let out = bind(project, label, me(), &lock.path, PROTOCOL).unwrap();
    (out.run, out.capability, lock)
}

/// A live reader registered under `handle` with role `role` and a
/// recipient UUID.
fn reader(project: &Path, handle: &str, role: &str) -> String {
    let entry = crate::agent_registry::register_existing_agent(
        project,
        "claude",
        me(),
        role.to_string(),
        None,
        project.to_path_buf(),
        Some(handle.to_string()),
        None,
    )
    .unwrap();
    agent_handle::reserve_handle(
        project,
        &agent_handle::HandleRequest {
            agent_type: "claude",
            explicit: Some(handle),
            registry_id: Some(&entry.id),
            ..Default::default()
        },
    )
    .unwrap()
    .recipient_id
}

fn directory(project: &Path) -> RecipientDirectory {
    RecipientDirectory::load(project, &project.join(".aida-store")).unwrap()
}

fn message(id: &str, from: &str, to: &str) -> aida_core::mailbox::Message {
    serde_json::from_value(serde_json::json!({
        "id": id, "thread_id": format!("thread-{id}"), "from": from,
        "to": {"kind": "agent", "agent": to},
        "timestamp": 10, "body": "private body", "subject": format!("subject {id}"),
    }))
    .unwrap()
}

/// Send through the shared resolver and delivery helper, as the CLI does.
fn send(project: &Path, id: &str, from: &str, to: &str) {
    let resolved = directory(project).resolve(to).unwrap();
    let (sidecar, _) = delivery_sidecar(&resolved, id, 10, false).unwrap();
    crate::mailbox_store::write_message_with_deliveries(
        project,
        &message(id, from, to),
        sidecar.as_ref(),
    )
    .unwrap();
}

fn sidecar_of(project: &Path, id: &str) -> Option<DeliverySidecar> {
    let path = aida_core::mailbox::local_mailbox_dir(project)
        .join(".deliveries")
        .join(format!("{id}.json"));
    let bytes = std::fs::read(path).ok()?;
    Some(DeliverySidecar::parse(&bytes, id).unwrap())
}

fn local_messages(project: &Path) -> Vec<aida_core::mailbox::Message> {
    crate::mailbox_store::read_local_messages(project).unwrap()
}

#[test]
fn mail_to_a_relay_label_is_recorded_as_a_delivery_to_its_run() {
    let tmp = tempfile::tempdir().unwrap();
    let (run, _cap, _lock) = bind_live(tmp.path(), "relay-a");
    let resolved = directory(tmp.path()).resolve("relay-a").unwrap();
    assert_eq!(
        resolved.relay_run.as_ref().map(|r| r.as_str()),
        Some(run.as_str())
    );
    send(tmp.path(), "m1", "sender-x", "relay-a");
    let sidecar = sidecar_of(tmp.path(), "m1").unwrap();
    let record = &sidecar.deliveries[0];
    assert_eq!(record.recipient_uuid.as_ref().unwrap().as_str(), run);
    assert_eq!(record.run_uuid.as_deref(), Some(run.as_str()));
    // Ordinary handle mail gets no delivery record.
    reader(tmp.path(), "plain-reader", "implementer");
    send(tmp.path(), "m2", "sender-x", "plain-reader");
    assert!(sidecar_of(tmp.path(), "m2").is_none());
}

#[test]
fn ending_a_run_tells_each_sender_once_and_never_bounces_a_bounce() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    let sender = reader(project, "sender-x", "implementer");
    let (run, cap, _lock) = bind_live(project, "relay-a");
    let run_uuid = RecipientUuid::parse(&run).unwrap();
    send(project, "m1", "sender-x", "relay-a");
    // An end notice that reached this run is never reported back.
    crate::mailbox_store::write_message_with_deliveries(
        project,
        &message("bounce", "relay-b", "relay-a"),
        Some(&DeliverySidecar::notice("bounce", &run_uuid, "earlier")),
    )
    .unwrap();

    outbox::LOCK_AT_MAIL_WRITE.with(|v| v.borrow_mut().clear());
    let ended = end(project, &run, &cap, TerminalReason::Stop).unwrap();
    let report = ended.notices.unwrap();
    assert_eq!(
        (report.inserted, report.delivered, report.pending),
        (0, 1, 1)
    );

    let entries = outbox::list_entries(project, &run).unwrap();
    assert_eq!(entries.len(), 2, "one sender and one advisor entry for m1");
    assert!(entries.iter().all(|e| e.message_id == "m1"));
    let sender_entry = entries
        .iter()
        .find(|e| e.destination.kind == DestinationKind::Sender)
        .unwrap();
    assert_eq!(
        sender_entry.destination.recipient.as_deref(),
        Some(sender.as_str())
    );
    let NoticeState::Delivered {
        message_id: Some(notice_id),
        ..
    } = &sender_entry.state
    else {
        panic!("sender notice not delivered: {sender_entry:?}");
    };
    // No advisor reader: the advisor notice stays visibly pending.
    let advisor = entries
        .iter()
        .find(|e| e.destination.kind == DestinationKind::Advisor)
        .unwrap();
    assert!(matches!(
        &advisor.state,
        NoticeState::Pending {
            attempts: 1,
            last_error: Some(_)
        }
    ));
    // A run with a notice still owed stays live, outbox and all.
    assert_eq!(report.archived, 0);
    assert!(runs_dir(project).join(format!("{run}.json")).exists());
    assert!(!ended_runs_dir(project).join(&run).exists());

    let notice = local_messages(project)
        .into_iter()
        .find(|m| &m.id == notice_id)
        .unwrap();
    assert_eq!(notice.in_reply_to.as_deref(), Some("m1"));
    assert_eq!(notice.thread_id, "thread-m1");
    assert!(notice.subject.unwrap().contains("subject m1"));
    assert!(!notice.body.contains("private body"));
    let marker = sidecar_of(project, notice_id).unwrap();
    assert_eq!(
        marker.deliveries[0].notice_of.as_deref(),
        Some(sender_entry.delivery_id.as_str())
    );
    assert_eq!(
        marker.deliveries[0]
            .recipient_uuid
            .as_ref()
            .unwrap()
            .as_str(),
        sender
    );

    // Retries and repeated ends insert nothing new and write no second
    // notice message.
    let count = local_messages(project).len();
    end(project, &run, &cap, TerminalReason::Stop).unwrap();
    outbox::settle_ended_runs(project).unwrap();
    assert_eq!(outbox::list_entries(project, &run).unwrap().len(), 2);
    assert_eq!(local_messages(project).len(), count);

    // F6: every notice mail write ran with the namespace lock released.
    let seen = outbox::LOCK_AT_MAIL_WRITE.with(|v| v.borrow().clone());
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|held| !held), "{seen:?}");
}

#[test]
fn an_advisor_who_sent_the_mail_is_told_once() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    reader(project, "advisor-one", "advisor");
    let (run, cap, _lock) = bind_live(project, "relay-a");
    send(project, "m1", "advisor-one", "relay-a");
    let ended = end(project, &run, &cap, TerminalReason::Exit).unwrap();
    // Every notice delivered: the run and its outbox leave the live records,
    // and the archived outbox still reads.
    assert_eq!(ended.notices.unwrap().archived, 1);
    assert!(!runs_dir(project).join(format!("{run}.json")).exists());
    assert!(!runs_dir(project).join(&run).exists());
    assert!(ended_runs_dir(project).join(format!("{run}.json")).exists());
    let entries = outbox::list_entries(project, &run).unwrap();
    assert_eq!(entries.len(), 2);
    for e in &entries {
        assert!(matches!(e.state, NoticeState::Delivered { .. }), "{e:?}");
    }
    let advisor = entries
        .iter()
        .find(|e| e.destination.kind == DestinationKind::Advisor)
        .unwrap();
    assert!(matches!(
        advisor.state,
        NoticeState::Delivered {
            message_id: None,
            ..
        }
    ));
    let notices = local_messages(project)
        .into_iter()
        .filter(|m| m.from == "relay-a")
        .count();
    assert_eq!(notices, 1);
}

#[test]
fn a_crashed_run_gets_its_notices_at_reconciliation_and_they_deliver_at_maintenance() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    reader(project, "sender-x", "implementer");
    let mut controller = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let lock = held_lock(project, "crash.lock", controller.id());
    let run = bind(
        project,
        "relay-crash",
        controller.id(),
        &lock.path,
        PROTOCOL,
    )
    .unwrap()
    .run;
    send(project, "m1", "sender-x", "relay-crash");
    controller.kill().unwrap();
    controller.wait().unwrap();

    // The send path's reconciliation records the end and inserts the
    // notices together, but never delivers them.
    let report = reconcile_relay_runs(project).unwrap();
    assert_eq!(report.ended, vec![run.clone()]);
    let entries = outbox::list_entries(project, &run).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|e| e.pending()));
    let terminal = load_run(project, &run).unwrap().unwrap().terminal.unwrap();
    assert!(entries
        .iter()
        .all(|e| e.terminal_generation == terminal.generation
            && e.terminal_reason == TerminalReason::Crash));

    let settled = outbox::settle_ended_runs(project).unwrap();
    assert_eq!(settled.delivered, 1);
    assert!(load_run(project, &run).unwrap().unwrap().notices_settled);
}

//! Reusing an ended relay run's label (STORY-1843 D2b): the exact unread
//! count, the refusal without a choice, leave-with-old, handover, and the
//! legacy-mail refusal.
// trace:STORY-1843 | ai:claude

use super::reuse::ReuseChoice;
use super::*;
use crate::agent_handle::load_reservation;
use crate::mailbox_resolver::{delivery_sidecar, RecipientDirectory};
use aida_core::mailbox_delivery::RecipientUuid;
use fs2::FileExt;
use std::io::Write;

struct HeldLock {
    _file: std::fs::File,
    path: PathBuf,
}

fn held_lock(dir: &Path, name: &str) -> HeldLock {
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(file, "{}", std::process::id()).unwrap();
    file.lock_exclusive().unwrap();
    HeldLock { _file: file, path }
}

fn message(id: &str, to: &str) -> aida_core::mailbox::Message {
    serde_json::from_value(serde_json::json!({
        "id": id, "thread_id": format!("thread-{id}"), "from": "sender-x",
        "to": {"kind": "agent", "agent": to},
        "timestamp": 10, "body": "body", "subject": format!("subject {id}"),
    }))
    .unwrap()
}

/// Send through the shared resolver and delivery helper, as the CLI does.
fn send(project: &Path, id: &str, to: &str) {
    let resolved = RecipientDirectory::load(project, &project.join(".aida-store"))
        .unwrap()
        .resolve(to)
        .unwrap();
    let (sidecar, _) = delivery_sidecar(&resolved, id, 10, false).unwrap();
    crate::mailbox_store::write_message_with_deliveries(
        project,
        &message(id, to),
        sidecar.as_ref(),
    )
    .unwrap();
}

/// The unread mail `run` owns under `label`, by count.
fn owned_unread(project: &Path, label: &str, run: &str) -> usize {
    let ctx = aida_core::mailbox_logical::MailboxContext::resolve_or_local(project, None).unwrap();
    let unread = aida_core::mailbox_logical::relay_run_unread(
        &ctx,
        label,
        &RecipientUuid::parse(run).unwrap(),
        &aida_core::mailbox_logical::ReadLimits::explicit_read(),
    );
    assert!(unread.complete);
    unread.owned
}

/// An ended run under `relay-a` with three unread messages, and the run
/// lock a new controller (this process) holds.
fn ended_with_unread(project: &Path) -> (String, HeldLock) {
    let lock = held_lock(project, "relay.lock");
    let first = bind(project, "relay-a", std::process::id(), &lock.path, PROTOCOL).unwrap();
    for id in ["m1", "m2", "m3"] {
        send(project, id, "relay-a");
    }
    end(project, &first.run, &first.capability, TerminalReason::Stop).unwrap();
    assert_eq!(owned_unread(project, "relay-a", &first.run), 3);
    (first.run, lock)
}

fn rebind(project: &Path, lock: &HeldLock, choice: Option<ReuseChoice>) -> Result<BindOutcome> {
    bind_reusing(
        project,
        "relay-a",
        std::process::id(),
        &lock.path,
        PROTOCOL,
        choice,
    )
}

#[test]
fn reuse_without_a_choice_refuses_with_the_exact_unread_count_and_reserves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (old, lock) = ended_with_unread(tmp.path());
    let before = std::fs::read_dir(runs_dir(tmp.path())).unwrap().count();
    let err = rebind(tmp.path(), &lock, None).unwrap_err().to_string();
    assert!(
        err.contains(&format!("relay run {old}, which has 3 unread messages")),
        "{err}"
    );
    assert!(
        err.contains("--leave-with-old") && err.contains("--handover"),
        "{err}"
    );
    assert_eq!(
        load_reservation(tmp.path(), "relay-a")
            .unwrap()
            .recipient_id,
        old
    );
    assert_eq!(
        std::fs::read_dir(runs_dir(tmp.path())).unwrap().count(),
        before
    );
}

#[test]
fn leave_with_old_keeps_the_mail_with_the_old_run() {
    let tmp = tempfile::tempdir().unwrap();
    let (old, lock) = ended_with_unread(tmp.path());
    let new = rebind(tmp.path(), &lock, Some(ReuseChoice::LeaveWithOld)).unwrap();
    let reuse = new.reuse.unwrap();
    assert_eq!((reuse.unread, reuse.handed_over), (3, 0));
    assert_eq!(owned_unread(tmp.path(), "relay-a", &new.run), 0);
    assert_eq!(owned_unread(tmp.path(), "relay-a", &old), 3);
    assert_eq!(
        load_reservation(tmp.path(), "relay-a")
            .unwrap()
            .recipient_id,
        new.run
    );
}

#[test]
fn handover_lets_the_new_run_read_the_mail_and_keeps_the_old_runs_ownership() {
    let tmp = tempfile::tempdir().unwrap();
    let (old, lock) = ended_with_unread(tmp.path());
    let new = rebind(tmp.path(), &lock, Some(ReuseChoice::Handover)).unwrap();
    let reuse = new.reuse.unwrap();
    assert_eq!((reuse.unread, reuse.handed_over), (3, 3));
    assert!(
        reuse.handover_errors.is_empty(),
        "{:?}",
        reuse.handover_errors
    );
    assert_eq!(owned_unread(tmp.path(), "relay-a", &new.run), 3);
    // Nothing moved or acknowledged: the old run still owns its deliveries.
    assert_eq!(owned_unread(tmp.path(), "relay-a", &old), 3);
}

#[test]
fn legacy_label_mail_refuses_reuse_whatever_the_choice() {
    let tmp = tempfile::tempdir().unwrap();
    let (_old, lock) = ended_with_unread(tmp.path());
    // Label mail with no delivery record is readable by whoever holds the
    // spelling.
    crate::mailbox_store::write_message_with_deliveries(
        tmp.path(),
        &message("legacy", "relay-a"),
        None,
    )
    .unwrap();
    for choice in [
        None,
        Some(ReuseChoice::LeaveWithOld),
        Some(ReuseChoice::Handover),
    ] {
        let err = rebind(tmp.path(), &lock, choice).unwrap_err().to_string();
        assert!(
            err.contains("1 unread message addressed to the label"),
            "{err}"
        );
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A Git project with a registered sibling worktree whose directory is
/// gone (missing or prunable).
fn project_with_missing_sibling(tmp: &Path) -> PathBuf {
    let project = tmp.join("main");
    std::fs::create_dir(&project).unwrap();
    git(&project, &["init", "-q"]);
    git(&project, &["commit", "-q", "--allow-empty", "-m", "init"]);
    git(&project, &["worktree", "add", "-q", "../sibling"]);
    std::fs::remove_dir_all(tmp.join("sibling")).unwrap();
    project
}

#[test]
fn a_missing_or_prunable_sibling_does_not_block_reuse_or_archiving() {
    let tmp = tempfile::tempdir().unwrap();
    let project = project_with_missing_sibling(tmp.path());
    // The fixture exercises the narrowed rule: Git lists the sibling as
    // prunable, and the relay count is still complete.
    let listed = crate::test_git::git_command()
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&listed.stdout).contains("prunable"));
    let ctx = aida_core::mailbox_logical::MailboxContext::resolve_or_local(&project, None).unwrap();
    assert!(
        aida_core::mailbox_logical::relay_run_unread(
            &ctx,
            "relay-a",
            &RecipientUuid::parse("00000000-0000-4000-8000-000000000001").unwrap(),
            &aida_core::mailbox_logical::ReadLimits::explicit_read(),
        )
        .complete
    );

    // An ended run with no unread mail settles and archives.
    let lock = held_lock(&project, "quiet.lock");
    let quiet = bind(
        &project,
        "relay-quiet",
        std::process::id(),
        &lock.path,
        PROTOCOL,
    )
    .unwrap();
    let ended = end(
        &project,
        &quiet.run,
        &quiet.capability,
        TerminalReason::Stop,
    )
    .unwrap();
    assert_eq!(ended.notices.unwrap().archived, 1);

    // A label whose ended run has unread mail is reused on an explicit choice.
    let (old, lock) = ended_with_unread(&project);
    let new = rebind(&project, &lock, Some(ReuseChoice::LeaveWithOld)).unwrap();
    assert_eq!(new.reuse.unwrap().previous_run, old);
}

fn session_req(label: &str) -> crate::agent_handle::HandleRequest<'_> {
    crate::agent_handle::HandleRequest {
        agent_type: "claude",
        explicit: Some(label),
        native_session_id: Some("sess-new"),
        registry_id: Some("sess-new"),
        ..Default::default()
    }
}

#[test]
fn a_session_taking_an_ended_runs_label_passes_the_unread_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let (old, _lock) = ended_with_unread(tmp.path());
    // A session cannot choose what happens to the old run's mail, so it is
    // refused like a headless reuse, and nothing is reserved.
    let err = crate::agent_handle::reserve_handle(tmp.path(), &session_req("relay-a"))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains(&format!("relay run {old}, which has 3 unread messages")),
        "{err}"
    );
    assert!(crate::agent_handle::preview_handle(tmp.path(), &session_req("relay-a")).is_err());
    assert_eq!(
        load_reservation(tmp.path(), "relay-a")
            .unwrap()
            .recipient_id,
        old
    );

    // An ended run with no unread mail gives its label up.
    let lock = held_lock(tmp.path(), "quiet.lock");
    let quiet = bind(
        tmp.path(),
        "relay-quiet",
        std::process::id(),
        &lock.path,
        PROTOCOL,
    )
    .unwrap();
    end(
        tmp.path(),
        &quiet.run,
        &quiet.capability,
        TerminalReason::Stop,
    )
    .unwrap();
    let taken =
        crate::agent_handle::reserve_handle(tmp.path(), &session_req("relay-quiet")).unwrap();
    assert_ne!(taken.recipient_id, quiet.run);
}

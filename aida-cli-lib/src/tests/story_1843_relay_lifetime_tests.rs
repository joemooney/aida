//! Relay mail lifetime (STORY-1843 D2a): one run UUID across rotations,
//! gap acceptance, terminal generation recorded once, crash reconciliation
//! that never retires an unknown controller.
// trace:STORY-1843 | ai:claude

use super::*;
use crate::agent_handle::{load_reservation, reserve_handle, HandleRequest};
use crate::mailbox_resolver::RecipientDirectory;
use fs2::FileExt;
use std::io::Write;

/// A run lock held the way relay.sh holds it: an exclusive flock on an open
/// file whose first line is the holder pid.
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

fn bind_live(project: &Path, label: &str) -> (BindOutcome, HeldLock) {
    let lock = held_lock(project, &format!("{label}.lock"), me());
    let out = bind(project, label, me(), &lock.path, PROTOCOL).unwrap();
    (out, lock)
}

/// A controller that has exited and been reaped: its PID is free.
fn dead_controller_run(project: &Path, label: &str) -> String {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let lock = held_lock(project, &format!("{label}.lock"), child.id());
    let out = bind(project, label, child.id(), &lock.path, PROTOCOL).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    out.run
}

fn session_req<'a>(label: &'a str, sid: &'a str, token: Option<&'a str>) -> HandleRequest<'a> {
    HandleRequest {
        agent_type: "claude",
        explicit: Some(label),
        native_session_id: Some(sid),
        registry_id: Some(sid),
        launch_token: token,
        ..Default::default()
    }
}

fn resolve(project: &Path, target: &str) -> Result<crate::mailbox_resolver::ResolvedRecipient> {
    RecipientDirectory::load(project, &project.join(".aida-store"))
        .unwrap()
        .resolve(target)
}

#[test]
fn a_protocol_mismatch_refuses_before_anything_is_reserved() {
    let tmp = tempfile::tempdir().unwrap();
    let lock = held_lock(tmp.path(), "relay.lock", me());
    let err = bind(tmp.path(), "relay-a", me(), &lock.path, "aida-relay-mail/0")
        .unwrap_err()
        .to_string();
    assert!(err.contains("not supported"), "{err}");
    assert!(load_reservation(tmp.path(), "relay-a").is_none());
    assert!(!runs_dir(tmp.path()).exists());
}

#[test]
fn bind_requires_the_controller_to_hold_its_run_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("free.lock");
    std::fs::write(&path, format!("{}\n", me())).unwrap();
    let err = bind(tmp.path(), "relay-a", me(), &path, PROTOCOL)
        .unwrap_err()
        .to_string();
    assert!(err.contains("does not hold"), "{err}");
    let other = held_lock(tmp.path(), "other.lock", me() + 1);
    let err = bind(tmp.path(), "relay-a", me(), &other.path, PROTOCOL)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not controller pid"), "{err}");
    assert!(load_reservation(tmp.path(), "relay-a").is_none());
}

#[test]
fn bind_reserves_the_label_with_the_run_uuid_and_holds_it_against_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, _lock) = bind_live(tmp.path(), "relay-a");
    let reservation = load_reservation(tmp.path(), "relay-a").unwrap();
    assert_eq!(reservation.recipient_id, out.run);
    let run = load_run(tmp.path(), &out.run).unwrap().unwrap();
    assert_eq!(run.label, "relay-a");
    assert_eq!(run.controller.pid, me());
    assert!(run.terminal.is_none());
    // Past the launch TTL the run still owns its label.
    let mut aged = reservation.clone();
    aged.reserved_at = chrono::DateTime::<Utc>::UNIX_EPOCH;
    agent_handle::write_reservation(tmp.path(), &aged).unwrap();
    let err = reserve_handle(tmp.path(), &session_req("relay-a", "stranger", None))
        .unwrap_err()
        .to_string();
    assert!(err.contains("relay run"), "{err}");
    let (_, _lock_b) = bind_live(tmp.path(), "relay-b");
    let err = bind(tmp.path(), "relay-a", me(), &_lock_b.path, PROTOCOL)
        .unwrap_err()
        .to_string();
    assert!(err.contains("in use by relay run"), "{err}");
}

#[test]
fn rotations_keep_one_run_uuid() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, _lock) = bind_live(tmp.path(), "relay-a");
    for (n, sid) in ["child-one", "child-two"].into_iter().enumerate() {
        let bound = child_bind(tmp.path(), &out.run, &out.capability, None).unwrap();
        let token = bound.launch_token.clone().unwrap();
        assert_eq!(bound.generation, 2 + n as u64);
        // An unrelated session without the token is refused the label.
        assert!(reserve_handle(tmp.path(), &session_req("relay-a", "intruder", None)).is_err());
        // The child's own startup presents the token and binds the label.
        let child = reserve_handle(tmp.path(), &session_req("relay-a", sid, Some(&token))).unwrap();
        assert_eq!(child.recipient_id, out.run);
        assert_eq!(child.native_session_id.as_deref(), Some(sid));
    }
    let direct = child_bind(tmp.path(), &out.run, &out.capability, Some("child-three")).unwrap();
    assert!(direct.launch_token.is_none());
    let r = load_reservation(tmp.path(), "relay-a").unwrap();
    assert_eq!(r.recipient_id, out.run);
    assert_eq!(r.native_session_id.as_deref(), Some("child-three"));
    assert!(r.previous_recipients.is_empty());
    assert_eq!(
        load_run(tmp.path(), &out.run)
            .unwrap()
            .unwrap()
            .children
            .len(),
        3
    );
}

#[test]
fn gap_mail_is_accepted_until_the_run_ends() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(resolve(tmp.path(), "relay-a").is_err());
    let (out, _lock) = bind_live(tmp.path(), "relay-a");
    let resolved = resolve(tmp.path(), "relay-a").unwrap();
    assert_eq!(resolved.address, "relay-a");
    assert!(resolved.reader_notice.unwrap().contains("between sessions"));
    end(tmp.path(), &out.run, &out.capability, TerminalReason::Stop).unwrap();
    let err = resolve(tmp.path(), "relay-a").unwrap_err().to_string();
    assert!(err.contains("unknown recipient"), "{err}");
}

#[test]
fn end_is_recorded_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, _lock) = bind_live(tmp.path(), "relay-a");
    let first = end(tmp.path(), &out.run, &out.capability, TerminalReason::Exit).unwrap();
    assert!(first.recorded_now);
    assert_eq!(first.terminal_generation, 2);
    let again = end(tmp.path(), &out.run, &out.capability, TerminalReason::Stop).unwrap();
    assert!(!again.recorded_now);
    assert_eq!(again.terminal_generation, 2);
    assert_eq!(again.reason, TerminalReason::Exit);
    let err = child_bind(tmp.path(), &out.run, &out.capability, None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("ended"), "{err}");
    // The ended record is archived out of the open-run directory.
    assert!(!run_path(tmp.path(), &out.run).exists());
    assert!(ended_run_path(tmp.path(), &out.run).exists());
}

/// Every file under `.aida/agents/`, so a refusal can be shown to write
/// nothing.
fn agents_snapshot(project: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(&agent_registry::agents_dir(project), &mut out);
    out
}

#[test]
fn bind_prints_the_capability_once_and_stores_only_its_digest() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, _lock) = bind_live(tmp.path(), "relay-a");
    assert_eq!(out.capability.len(), 2 * CAPABILITY_BYTES);
    assert!(out.capability.bytes().all(|b| b.is_ascii_hexdigit()));
    let (other, _lock_b) = bind_live(tmp.path(), "relay-b");
    assert_ne!(out.capability, other.capability);
    for (path, body) in agents_snapshot(tmp.path()) {
        let body = String::from_utf8_lossy(&body);
        assert!(
            !body.contains(&out.capability),
            "{} stores the capability",
            path.display()
        );
    }
    let run = load_run(tmp.path(), &out.run).unwrap().unwrap();
    assert_eq!(run.capability_sha256, capability_digest(&out.capability));
}

#[test]
fn child_bind_and_end_refuse_a_caller_without_the_capability_and_write_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, _lock) = bind_live(tmp.path(), "relay-a");
    // The legitimate child holds the label.
    child_bind(tmp.path(), &out.run, &out.capability, Some("real-child")).unwrap();
    let before = agents_snapshot(tmp.path());
    let wrong = "0".repeat(2 * CAPABILITY_BYTES);
    // The run id and controller pid are public; neither grants anything.
    let foreign_token = child_bind(tmp.path(), &out.run, &wrong, None).unwrap_err();
    let foreign_session =
        child_bind(tmp.path(), &out.run, &wrong, Some("intruder-session")).unwrap_err();
    let foreign_end = end(tmp.path(), &out.run, &wrong, TerminalReason::Stop).unwrap_err();
    let empty = end(tmp.path(), &out.run, "", TerminalReason::Stop).unwrap_err();
    let other_run = bind_live(tmp.path(), "relay-b");
    let after_bind_b = agents_snapshot(tmp.path());
    let cross = end(
        tmp.path(),
        &out.run,
        &other_run.0.capability,
        TerminalReason::Stop,
    )
    .unwrap_err();
    for err in [foreign_token, foreign_session, foreign_end, empty, cross] {
        assert!(
            err.to_string().contains("capability does not match"),
            "{err}"
        );
    }
    let snapshot = agents_snapshot(tmp.path());
    assert_eq!(snapshot, after_bind_b);
    for (path, body) in &before {
        assert_eq!(snapshot.get(path), Some(body), "{} changed", path.display());
    }
    let r = load_reservation(tmp.path(), "relay-a").unwrap();
    assert_eq!(r.native_session_id.as_deref(), Some("real-child"));
    assert!(load_run(tmp.path(), &out.run)
        .unwrap()
        .unwrap()
        .terminal
        .is_none());
}

#[test]
fn a_record_without_a_digest_accepts_no_capability() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, _lock) = bind_live(tmp.path(), "relay-a");
    let mut run = load_run(tmp.path(), &out.run).unwrap().unwrap();
    run.capability_sha256.clear();
    write_run(tmp.path(), &run).unwrap();
    assert!(end(tmp.path(), &out.run, &out.capability, TerminalReason::Stop).is_err());
    assert!(end(tmp.path(), &out.run, "", TerminalReason::Stop).is_err());
}

#[test]
fn the_capability_file_must_be_private_and_not_a_symlink() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("cap");
    std::fs::write(&path, "abc123\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(read_capability(Some(&path)).unwrap(), "abc123");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = read_capability(Some(&path)).unwrap_err().to_string();
    assert!(err.contains("0600"), "{err}");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(read_capability(Some(&link)).is_err());
    let empty = tmp.path().join("empty");
    std::fs::write(&empty, "\n").unwrap();
    std::fs::set_permissions(&empty, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(read_capability(Some(&empty)).is_err());
}

/// An open-run record as bind would write it, for budget tests.
fn synthetic_run(template: &RelayRun, ended: bool) -> RelayRun {
    let mut run = template.clone();
    run.run_id = uuid::Uuid::new_v4().to_string();
    run.label = format!("relay-{}", &run.run_id[..8]);
    run.terminal = ended.then(|| Terminal {
        generation: 2,
        reason: TerminalReason::Exit,
        at: Utc::now(),
    });
    run.notices_settled = ended;
    run
}

#[test]
fn ended_runs_and_stray_entries_do_not_count_against_the_open_run_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, _lock) = bind_live(tmp.path(), "relay-a");
    let template = load_run(tmp.path(), &out.run).unwrap().unwrap();
    for _ in 0..MAX_OPEN_RUN_RECORDS + 8 {
        archive_run(tmp.path(), &synthetic_run(&template, true)).unwrap();
    }
    for n in 0..8 {
        std::fs::write(runs_dir(tmp.path()).join(format!("stray-{n}.tmp")), "x").unwrap();
    }
    // The cliff: open records up to the budget, the live run included.
    for _ in 1..MAX_OPEN_RUN_RECORDS {
        write_run(tmp.path(), &synthetic_run(&template, false)).unwrap();
    }
    assert_eq!(list_runs(tmp.path()).unwrap().len(), MAX_OPEN_RUN_RECORDS);
    let (accepting, warnings) = accepting_runs(tmp.path());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(accepting.iter().any(|r| r.run_id == out.run));
    reconcile_relay_runs(tmp.path()).unwrap();
    // One more open record is over it: listing refuses, gap acceptance warns.
    write_run(tmp.path(), &synthetic_run(&template, false)).unwrap();
    let err = list_runs(tmp.path()).unwrap_err().to_string();
    assert!(err.contains("live relay run budget"), "{err}");
    let (accepting, warnings) = accepting_runs(tmp.path());
    assert!(accepting.is_empty());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

#[test]
fn reconcile_ends_a_dead_controller_and_never_an_unknown_one() {
    let tmp = tempfile::tempdir().unwrap();
    let (live, _lock) = bind_live(tmp.path(), "relay-live");
    let (unknown, _lock_u) = bind_live(tmp.path(), "relay-unknown");
    // Created last: every bind is itself a reconciliation point.
    let dead = dead_controller_run(tmp.path(), "relay-dead");
    // Stamp a PID namespace this process cannot observe.
    let mut run = load_run(tmp.path(), &unknown.run).unwrap().unwrap();
    run.controller.pid_namespace = Some(1);
    write_run(tmp.path(), &run).unwrap();
    assert_eq!(
        controller_liveness(&run.controller),
        ControllerLiveness::Unknown
    );

    let report = reconcile_relay_runs(tmp.path()).unwrap();
    // Ended: the dead run no longer accepts gap mail.
    assert!(resolve(tmp.path(), "relay-dead").is_err());
    assert_eq!(report.ended, vec![dead.clone()]);
    assert_eq!(report.unknown, vec![unknown.run.clone()]);
    let ended = load_run(tmp.path(), &dead).unwrap().unwrap();
    assert_eq!(ended.terminal.unwrap().reason, TerminalReason::Crash);
    assert!(load_run(tmp.path(), &live.run)
        .unwrap()
        .unwrap()
        .terminal
        .is_none());
    assert!(load_run(tmp.path(), &unknown.run)
        .unwrap()
        .unwrap()
        .terminal
        .is_none());
    // Unknown liveness still accepts, with a warning.
    let notice = resolve(tmp.path(), "relay-unknown")
        .unwrap()
        .reader_notice
        .unwrap();
    assert!(notice.starts_with("warning:"), "{notice}");
    // A second pass records nothing new.
    assert!(reconcile_relay_runs(tmp.path()).unwrap().ended.is_empty());
}

#[test]
fn a_reused_controller_pid_is_dead() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, _lock) = bind_live(tmp.path(), "relay-a");
    let mut run = load_run(tmp.path(), &out.run).unwrap().unwrap();
    run.controller.start_identity = "linux-starttime:1".to_string();
    assert_eq!(
        controller_liveness(&run.controller),
        ControllerLiveness::Dead
    );
    run.controller.start_identity = load_run(tmp.path(), &out.run)
        .unwrap()
        .unwrap()
        .controller
        .start_identity;
    run.controller.boot_id = Some("another-boot".to_string());
    if current_boot_id().is_some() {
        assert_eq!(
            controller_liveness(&run.controller),
            ControllerLiveness::Dead
        );
    }
}

#[test]
fn an_ended_runs_label_with_no_unread_mail_is_reused_by_a_new_run() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, lock) = bind_live(tmp.path(), "relay-a");
    end(tmp.path(), &out.run, &out.capability, TerminalReason::Stop).unwrap();
    let again = bind(tmp.path(), "relay-a", me(), &lock.path, PROTOCOL).unwrap();
    assert_ne!(again.run, out.run);
    let reuse = again.reuse.unwrap();
    assert_eq!(
        (reuse.previous_run.as_str(), reuse.unread),
        (out.run.as_str(), 0)
    );
    let r = load_reservation(tmp.path(), "relay-a").unwrap();
    assert_eq!(r.recipient_id, again.run);
    assert_eq!(r.previous_recipients, vec![out.run]);
}

#[test]
fn run_ids_must_be_canonical() {
    let tmp = tempfile::tempdir().unwrap();
    for bad in ["../x", "not-a-uuid", "6F9619FF-8B86-D011-B42D-00C04FC964FF"] {
        assert!(child_bind(tmp.path(), bad, "x", None).is_err(), "{bad}");
        assert!(
            end(tmp.path(), bad, "x", TerminalReason::Stop).is_err(),
            "{bad}"
        );
    }
}

//! C1 fixtures for the mail-source lifecycle (C SKETCH v1 section 6).
//! trace:STORY-1843 | ai:claude

use super::retire::{
    complete_retirement, retire_source, CopyFault, JournalEntry, Manifest, Phase, Point,
    RefusalReason, RetireOptions, RetireOutcome, COPY_FAULT, CRASH_AT, HOOK_AT,
};
use super::*;
use crate::mailbox::{
    local_mailbox_dir, write_local_message, Intent, Message, Recipient, SenderSource,
};
use crate::process_probe::fixture::FakeProc;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

fn repo() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let out = crate::test_git::git_command()
        .args(["init", "-q"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    dir
}

fn message(id: &str) -> Message {
    Message {
        subject: None,
        id: id.to_string(),
        thread_id: id.to_string(),
        from: "alice".into(),
        to: Recipient::Agent("bob".into()),
        timestamp: 1,
        in_reply_to: None,
        body: format!("body-{id}"),
        urgent: false,
        intent: Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    }
}

fn mailbox(root: &Path) -> PathBuf {
    local_mailbox_dir(root)
}

fn ensure_mailbox(root: &Path) {
    std::fs::create_dir_all(mailbox(root)).unwrap();
}

fn source(root: &Path) -> Source {
    ensure_mailbox(root);
    resolve_source(root)
        .unwrap()
        .expect("a git checkout has an anchor")
}

fn record(root: &Path) -> SourceRecord {
    let source = source(root);
    let held = lock(&source.anchor, LIFECYCLE_LOCK_TIMEOUT).unwrap();
    load_or_create(&source, &held).unwrap()
}

fn edit_record(root: &Path, f: impl FnOnce(&mut SourceRecord)) {
    let source = source(root);
    let held = lock(&source.anchor, LIFECYCLE_LOCK_TIMEOUT).unwrap();
    let mut record = load_or_create(&source, &held).unwrap();
    f(&mut record);
    store_record(&source.anchor, &record, &held).unwrap();
}

fn activate(root: &Path) {
    let anchor = source(root).anchor;
    write_json(
        &anchor.join(ACTIVATION_FILE),
        &Activation {
            schema: SCHEMA,
            all_writer_hosts_admitting: true,
            recorded_at: "2026-10-26T00:00:00Z".into(),
        },
    )
    .unwrap();
}

/// Messages plus a watermark and a receipt, as a real mailbox holds them.
fn seed_mail(root: &Path) {
    write_local_message(root, &message("m1")).unwrap();
    write_local_message(root, &message("m2")).unwrap();
    let read = mailbox(root).join(".read");
    std::fs::create_dir_all(&read).unwrap();
    std::fs::write(read.join("bob.txt"), "1").unwrap();
    let receipts = mailbox(root).join(".read-receipts/bob");
    std::fs::create_dir_all(&receipts).unwrap();
    std::fs::write(receipts.join("m1.txt"), "2").unwrap();
}

const SEEDED: [&str; 4] = [
    ".read-receipts/bob/m1.txt",
    ".read/bob.txt",
    "m1.json",
    "m2.json",
];

/// Relative paths of every file under `dir`, sorted.
fn files_under(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for item in read {
            let path = item.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn opts() -> RetireOptions {
    RetireOptions {
        quiesce_timeout: Duration::from_millis(500),
        ..RetireOptions::default()
    }
}

fn retired(outcome: RetireOutcome) -> (super::retire::RetirementTicket, PathBuf) {
    match outcome {
        RetireOutcome::Retired {
            ticket,
            retained_dir,
            ..
        } => (ticket, retained_dir),
        other => panic!("expected Retired, got {other:?}"),
    }
}

fn refusal(outcome: RetireOutcome) -> (RefusalReason, Option<PathBuf>) {
    match outcome {
        RetireOutcome::Refused {
            reason,
            retained_dir,
        } => (reason, retained_dir),
        other => panic!("expected Refused, got {other:?}"),
    }
}

fn journal(ticket: &super::retire::RetirementTicket) -> JournalEntry {
    read_json(
        &ticket
            .anchor
            .join(JOURNAL_DIR)
            .join(format!("{}.json", ticket.op_id)),
    )
    .unwrap()
    .unwrap()
}

fn assert_retiring(root: &Path) {
    match admit_local_write(root) {
        Err(LifecycleError::SourceRetiring { .. }) => {}
        other => panic!("expected SourceRetiring, got {other:?}"),
    }
}

fn assert_open(root: &Path) {
    let permit = admit_local_write(root).unwrap().expect("admitted");
    permit.complete().unwrap();
    assert_eq!(record(root).state, SourceState::Open);
}

// ── Admission ───────────────────────────────────────────────────────────────

#[test]
fn local_write_is_admitted_and_releases_its_permit() {
    let dir = repo();
    let ((), classes) =
        observe_lock_classes(|| write_local_message(dir.path(), &message("m1")).unwrap());
    // Git resolves the anchor before any lock, then two short leaf holds
    // (admit, complete); nothing is entered under either.
    assert_eq!(classes, vec!["git", "lifecycle", "lifecycle"]);
    // The resolved anchor is cached for the process: no Git on later writes.
    let ((), classes) =
        observe_lock_classes(|| write_local_message(dir.path(), &message("m2")).unwrap());
    assert_eq!(classes, vec!["lifecycle", "lifecycle"]);
    assert!(mailbox(dir.path()).join("m1.json").is_file());
    let record = record(dir.path());
    assert_eq!(record.state, SourceState::Open);
    assert!(record.permits.is_empty());
    assert!(record.generation >= 1);
}

#[test]
fn a_live_permit_carries_the_writer_identity_and_generation() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let permit = admit_local_write(dir.path()).unwrap().unwrap();
    let record = record(dir.path());
    assert_eq!(record.permits.len(), 1);
    assert_eq!(record.permits[0].kind, PermitKind::Local);
    assert_eq!(record.permits[0].generation, record.generation);
    assert_eq!(
        record.permits[0].holder.as_ref().map(|h| h.pid),
        Some(std::process::id())
    );
    drop(permit);
    assert!(super::tests::record(dir.path()).permits.is_empty());
}

#[test]
fn a_checkout_outside_git_writes_without_an_anchor() {
    let dir = tempfile::tempdir().unwrap();
    write_local_message(dir.path(), &message("m1")).unwrap();
    assert!(mailbox(dir.path()).join("m1.json").is_file());
    assert!(resolve_source(dir.path()).unwrap().is_none());
}

#[test]
fn the_anchor_lives_in_the_common_git_dir_and_is_private() {
    let dir = repo();
    let source = source(dir.path());
    let common = std::fs::canonicalize(dir.path().join(".git")).unwrap();
    assert_eq!(source.anchor, common.join("aida/mail-lifecycle"));
    assert_eq!(source.admin, "");
    for sub in ["", SOURCES_DIR, RETAINED_DIR, JOURNAL_DIR] {
        let mode = std::fs::metadata(source.anchor.join(sub))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "{sub}");
    }
}

#[test]
fn a_group_writable_common_git_dir_keeps_local_mail_working() {
    // Under umask 002 Git makes the repository's dirs 0775. That mode is the
    // repository's, not the anchor's: mail is admitted and the anchor AIDA
    // creates is still private.
    let dir = repo();
    for path in [dir.path().to_path_buf(), dir.path().join(".git")] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o775)).unwrap();
    }
    write_local_message(dir.path(), &message("m1")).unwrap();
    assert!(mailbox(dir.path()).join("m1.json").is_file());
    let source = source(dir.path());
    for sub in ["", SOURCES_DIR] {
        let mode = std::fs::metadata(source.anchor.join(sub))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "{sub}");
    }
    let record_mode = std::fs::metadata(record_path(&source.anchor, &source.key))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(record_mode & 0o777, 0o600);
    assert!(matches!(
        cover_local_write(dir.path()).unwrap(),
        RetentionCoverage::Admitted(_)
    ));
}

#[test]
fn an_unusable_anchor_delivers_uncovered_and_retirement_refuses() {
    let dir = repo();
    write_local_message(dir.path(), &message("m1")).unwrap();
    let source = source(dir.path());
    let path = record_path(&source.anchor, &source.key);

    // A damaged record: delivery is unchanged, coverage stays unestablished.
    std::fs::write(&path, "not json").unwrap();
    write_local_message(dir.path(), &message("m2")).unwrap();
    assert!(mailbox(dir.path()).join("m2.json").is_file());
    assert!(matches!(
        cover_local_write(dir.path()).unwrap(),
        RetentionCoverage::Unestablished(Some(_))
    ));
    // Retirement is the side that fails closed.
    assert!(matches!(
        retire_source(dir.path(), &opts()),
        Err(LifecycleError::Corrupt { .. })
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"not json");

    // An anchor AIDA can no longer trust: the same.
    std::fs::remove_file(&path).unwrap();
    std::fs::set_permissions(&source.anchor, std::fs::Permissions::from_mode(0o755)).unwrap();
    write_local_message(dir.path(), &message("m3")).unwrap();
    assert!(mailbox(dir.path()).join("m3.json").is_file());
    let (reason, _) = refusal(retire_source(dir.path(), &opts()).unwrap());
    assert!(
        matches!(reason, RefusalReason::UnsafeMailbox { .. }),
        "{reason:?}"
    );
    std::fs::set_permissions(&source.anchor, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn a_linked_worktree_is_its_own_source_under_the_shared_anchor() {
    let dir = repo();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(dir.path())
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    let wt = dir.path().join("wt");
    git(&["worktree", "add", "-q", wt.to_str().unwrap()]);
    let main = source(dir.path());
    let linked = source(&wt);
    assert_eq!(main.anchor, linked.anchor);
    assert_eq!(linked.admin, "wt");
    assert_ne!(main.key, linked.key);
}

#[test]
fn unsafe_record_files_are_refused() {
    let dir = repo();
    write_local_message(dir.path(), &message("m1")).unwrap();
    let source = source(dir.path());
    let path = record_path(&source.anchor, &source.key);
    let real = source.anchor.join("elsewhere.json");
    std::fs::rename(&path, &real).unwrap();

    std::os::unix::fs::symlink(&real, &path).unwrap();
    assert!(matches!(
        admit_local_write(dir.path()),
        Err(LifecycleError::Unsafe { .. })
    ));
    std::fs::remove_file(&path).unwrap();

    std::fs::hard_link(&real, &path).unwrap();
    assert!(matches!(
        admit_local_write(dir.path()),
        Err(LifecycleError::Unsafe { .. })
    ));
    std::fs::remove_file(&path).unwrap();

    let fifo = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(matches!(
        admit_local_write(dir.path()),
        Err(LifecycleError::Unsafe { .. })
    ));
}

#[test]
fn a_symlinked_lock_file_is_refused() {
    let dir = repo();
    let source = source(dir.path());
    std::os::unix::fs::symlink("/dev/null", source.anchor.join(LOCK_FILE)).unwrap();
    assert!(matches!(
        lock(&source.anchor, LIFECYCLE_LOCK_TIMEOUT),
        Err(LifecycleError::Unsafe { .. })
    ));
}

// ── Lock discipline ─────────────────────────────────────────────────────────

#[test]
fn the_lifecycle_lock_is_a_bounded_try() {
    let dir = repo();
    let anchor = source(dir.path()).anchor;
    let (taken_tx, taken_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder_anchor = anchor.clone();
    let holder = std::thread::spawn(move || {
        let _held = lock(&holder_anchor, LIFECYCLE_LOCK_TIMEOUT).unwrap();
        taken_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    taken_rx.recv().unwrap();
    let start = Instant::now();
    assert!(matches!(
        lock(&anchor, Duration::from_millis(150)),
        Err(LifecycleError::LockTimeout { .. })
    ));
    assert!(start.elapsed() < Duration::from_secs(2));
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    drop(lock(&anchor, LIFECYCLE_LOCK_TIMEOUT).unwrap());
}

#[test]
fn the_lifecycle_lock_is_a_leaf_in_both_directions() {
    let dir = repo();
    let anchor = source(dir.path()).anchor;
    let held = lock(&anchor, LIFECYCLE_LOCK_TIMEOUT).unwrap();
    assert!(lifecycle_lock_held());
    assert!(matches!(
        lock(&anchor, LIFECYCLE_LOCK_TIMEOUT),
        Err(LifecycleError::LockOrder(_))
    ));
    let store = tempfile::tempdir().unwrap();
    let err = crate::db::with_store_write_lock(store.path(), || Ok(())).unwrap_err();
    assert!(err.to_string().contains("lifecycle lock"), "{err}");
    drop(held);
    assert!(!lifecycle_lock_held());

    let inside = crate::db::with_store_write_lock(store.path(), || {
        Ok(matches!(
            lock(&anchor, LIFECYCLE_LOCK_TIMEOUT),
            Err(LifecycleError::LockOrder(_))
        ))
    });
    assert!(
        inside.unwrap(),
        "a Store-held region must not take Lifecycle"
    );
}

#[test]
fn git_and_the_pool_lock_never_start_under_lifecycle() {
    let dir = repo();
    let anchor = source(dir.path()).anchor;
    let root = dir.path().to_path_buf();
    // The assertion is a debug assertion; the class is recorded either way.
    if cfg!(debug_assertions) {
        let held = lock(&anchor, LIFECYCLE_LOCK_TIMEOUT).unwrap();
        let git = std::panic::catch_unwind(|| crate::git_ops::host_git_command(&root));
        assert!(git.is_err(), "Git built under the Lifecycle lock");
        let pool =
            std::panic::catch_unwind(|| crate::worktree_pool::with_state_lock(&root, |_| Ok(())));
        assert!(
            pool.is_err(),
            "the pool lock taken under the Lifecycle lock"
        );
        drop(held);
    }
    let ((), classes) = observe_lock_classes(|| {
        drop(crate::git_ops::host_git_command(&root));
    });
    assert_eq!(classes, vec!["git"]);
}

// ── Permit rows ─────────────────────────────────────────────────────────────

fn dead_identity() -> ProcessIdentity {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let identity = ProbeSource::system().capture(child.id()).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    identity
}

#[test]
fn admission_reaps_rows_whose_writer_is_proven_dead() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let dead = dead_identity();
    edit_record(dir.path(), |r| {
        r.permits = vec![
            Permit {
                permit_id: "dead".into(),
                kind: PermitKind::Local,
                holder: Some(dead.clone()),
                generation: r.generation,
            },
            Permit {
                permit_id: "unknown".into(),
                kind: PermitKind::Local,
                holder: None,
                generation: r.generation,
            },
        ]
    });
    let permit = admit_local_write(dir.path()).unwrap().unwrap();
    let ids: Vec<String> = record(dir.path())
        .permits
        .into_iter()
        .map(|p| p.permit_id)
        .collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert_eq!(ids[0], "unknown");
    permit.complete().unwrap();
    assert_eq!(record(dir.path()).permits.len(), 1);
}

#[test]
fn a_failed_release_keeps_the_row_for_drop_to_retry() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let anchor = source(dir.path()).anchor;
    let permit = admit_local_write(dir.path()).unwrap().unwrap();
    let (taken_tx, taken_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _held = lock(&anchor, LIFECYCLE_LOCK_TIMEOUT).unwrap();
        taken_tx.send(()).unwrap();
        // Past complete()'s bounded try, inside Drop's retry.
        std::thread::sleep(LIFECYCLE_LOCK_TIMEOUT + Duration::from_millis(600));
    });
    taken_rx.recv().unwrap();
    assert!(matches!(
        permit.complete(),
        Err(LifecycleError::LockTimeout { .. })
    ));
    holder.join().unwrap();
    assert!(record(dir.path()).permits.is_empty());
}

// ── Retirement ──────────────────────────────────────────────────────────────

#[test]
fn an_empty_source_retires_and_completes_after_removal() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let (ticket, retained) = retired(retire_source(dir.path(), &opts()).unwrap());
    assert_retiring(dir.path());
    let manifest: Manifest = read_json(&retained.join("manifest.json")).unwrap().unwrap();
    assert_eq!(manifest.count, 0);

    assert!(matches!(
        complete_retirement(&ticket),
        Err(LifecycleError::NotRemoved { .. })
    ));
    std::fs::remove_dir_all(dir.path().join(".aida")).unwrap();
    complete_retirement(&ticket).unwrap();
    assert_eq!(journal(&ticket).phase, Phase::Finalized);
    assert!(!record_path(&ticket.anchor, &journal(&ticket).key).exists());
    complete_retirement(&ticket).unwrap();
}

#[test]
fn an_absent_mailbox_is_created_so_it_can_be_fenced() {
    let dir = repo();
    let (_, _) = retired(retire_source(dir.path(), &opts()).unwrap());
    assert!(mailbox(dir.path()).is_dir());
    assert_retiring(dir.path());
}

#[test]
fn a_mail_bearing_source_refuses_before_activation_and_reopens() {
    let dir = repo();
    seed_mail(dir.path());
    let (reason, retained) = refusal(retire_source(dir.path(), &opts()).unwrap());
    assert_eq!(reason, RefusalReason::MailBearingBeforeActivation);
    assert!(reason.to_string().contains(ACTIVATION_COMMAND));
    assert_eq!(retained, None);
    assert_eq!(files_under(&mailbox(dir.path())), SEEDED);
    assert_open(dir.path());
}

#[test]
fn even_an_empty_file_makes_a_source_mail_bearing() {
    let dir = repo();
    ensure_mailbox(dir.path());
    std::fs::create_dir_all(mailbox(dir.path()).join(".read")).unwrap();
    std::fs::write(mailbox(dir.path()).join(".read/x.txt"), "").unwrap();
    let (reason, _) = refusal(retire_source(dir.path(), &opts()).unwrap());
    assert_eq!(reason, RefusalReason::MailBearingBeforeActivation);
}

#[test]
fn an_activated_source_preserves_mail_receipts_and_watermarks() {
    let dir = repo();
    seed_mail(dir.path());
    activate(dir.path());
    let before = record(dir.path()).generation;
    let (ticket, retained) = retired(retire_source(dir.path(), &opts()).unwrap());
    assert_eq!(files_under(&mailbox(dir.path())), Vec::<String>::new());
    assert_eq!(files_under(&retained.join("data")), SEEDED);
    let manifest: Manifest = read_json(&retained.join("manifest.json")).unwrap().unwrap();
    assert_eq!(manifest.count, SEEDED.len());
    assert_eq!(manifest.generation, before);
    assert_eq!(manifest.original_path, mailbox(dir.path()));
    let m1 = manifest
        .files
        .iter()
        .find(|f| f.path == Path::new("m1.json"))
        .unwrap();
    let bytes = std::fs::read(retained.join("data/m1.json")).unwrap();
    assert_eq!(m1.sha256, hex(&Sha256::digest(&bytes)));
    assert_eq!(journal(&ticket).phase, Phase::Committed);
    assert!(matches!(
        record(dir.path()).state,
        SourceState::Retired { .. }
    ));
}

#[test]
fn a_cross_device_copy_preserves_and_verifies() {
    let dir = repo();
    seed_mail(dir.path());
    activate(dir.path());
    let opts = RetireOptions {
        force_copy: true,
        ..opts()
    };
    let (_, retained) = retired(retire_source(dir.path(), &opts).unwrap());
    assert_eq!(files_under(&retained.join("data")), SEEDED);
    assert_eq!(files_under(&mailbox(dir.path())), Vec::<String>::new());
}

#[test]
fn a_cross_device_manifest_mismatch_refuses_and_keeps_the_source() {
    let dir = repo();
    seed_mail(dir.path());
    activate(dir.path());
    COPY_FAULT.with(|f| f.set(Some(CopyFault::Corrupt)));
    let opts = RetireOptions {
        force_copy: true,
        ..opts()
    };
    let outcome = retire_source(dir.path(), &opts);
    COPY_FAULT.with(|f| f.set(None));
    let (reason, _) = refusal(outcome.unwrap());
    assert!(
        matches!(reason, RefusalReason::ManifestMismatch { .. }),
        "{reason:?}"
    );
    assert_eq!(files_under(&mailbox(dir.path())), SEEDED);
    assert_open(dir.path());
}

#[test]
fn a_full_disk_refuses_and_keeps_the_source() {
    let dir = repo();
    seed_mail(dir.path());
    activate(dir.path());
    COPY_FAULT.with(|f| f.set(Some(CopyFault::DiskFull)));
    let opts = RetireOptions {
        force_copy: true,
        ..opts()
    };
    let outcome = retire_source(dir.path(), &opts);
    COPY_FAULT.with(|f| f.set(None));
    let (reason, _) = refusal(outcome.unwrap());
    assert!(
        matches!(reason, RefusalReason::PreserveFailed { .. }),
        "{reason:?}"
    );
    assert_eq!(files_under(&mailbox(dir.path())), SEEDED);
    assert_open(dir.path());
}

#[test]
fn a_symlink_in_the_mailbox_refuses_preservation() {
    let dir = repo();
    seed_mail(dir.path());
    activate(dir.path());
    std::os::unix::fs::symlink("/etc/hostname", mailbox(dir.path()).join("zz.json")).unwrap();
    let (reason, _) = refusal(retire_source(dir.path(), &opts()).unwrap());
    assert!(
        matches!(reason, RefusalReason::PreserveFailed { .. }),
        "{reason:?}"
    );
    let mut expected: Vec<String> = SEEDED.iter().map(|s| s.to_string()).collect();
    expected.push("zz.json".into());
    assert_eq!(files_under(&mailbox(dir.path())), expected);
    assert_open(dir.path());
}

// ── Writers around the fence ────────────────────────────────────────────────

#[test]
fn a_writer_admitted_before_the_fence_finishes_and_is_captured() {
    let dir = repo();
    seed_mail(dir.path());
    activate(dir.path());
    let permit = admit_local_write(dir.path()).unwrap().unwrap();
    let root = dir.path().to_path_buf();
    let retirer = std::thread::spawn(move || {
        let opts = RetireOptions {
            quiesce_timeout: Duration::from_secs(20),
            ..RetireOptions::default()
        };
        retire_source(&root, &opts)
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !matches!(record(dir.path()).state, SourceState::Fenced { .. }) {
        assert!(Instant::now() < deadline, "the retirer never fenced");
        std::thread::sleep(Duration::from_millis(10));
    }
    // The admitted writer finishes its write after the fence...
    std::fs::write(mailbox(dir.path()).join("m3.json"), "{}").unwrap();
    permit.complete().unwrap();
    // ...and the barrier waited for it, so the write is preserved.
    let (_, retained) = retired(retirer.join().unwrap().unwrap());
    assert!(retained.join("data/m3.json").is_file());
    assert_eq!(files_under(&mailbox(dir.path())), Vec::<String>::new());
}

#[test]
fn a_writer_after_the_fence_is_refused() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let root = dir.path().to_path_buf();
    let refused = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = refused.clone();
    HOOK_AT.with(|h| {
        *h.borrow_mut() = Some((
            Point::AfterFence,
            Box::new(move || {
                let err = write_local_message(&root, &message("late")).unwrap_err();
                let inner = err
                    .get_ref()
                    .and_then(|e| e.downcast_ref::<LifecycleError>());
                seen.set(matches!(inner, Some(LifecycleError::SourceRetiring { .. })));
            }),
        ))
    });
    let outcome = retire_source(dir.path(), &opts()).unwrap();
    assert!(refused.get(), "the post-fence write was not refused");
    let (_, retained) = retired(outcome);
    assert!(!mailbox(dir.path()).join("late.json").exists());
    assert_eq!(files_under(&retained.join("data")), Vec::<String>::new());
}

#[test]
fn a_non_admitting_late_writer_is_caught_and_everything_is_kept() {
    let dir = repo();
    seed_mail(dir.path());
    activate(dir.path());
    let late = mailbox(dir.path()).join("late.json");
    let late_path = late.clone();
    HOOK_AT.with(|h| {
        *h.borrow_mut() = Some((
            Point::BeforeFinalCheck,
            Box::new(move || std::fs::write(&late_path, "{}").unwrap()),
        ))
    });
    let (reason, retained) = refusal(retire_source(dir.path(), &opts()).unwrap());
    assert_eq!(
        reason,
        RefusalReason::LateWriter {
            entries: vec!["late.json".into()]
        }
    );
    // The tree keeps the late write and gets the preserved bytes back; the
    // retained copy is kept too.
    let mut expected: Vec<String> = SEEDED.iter().map(|s| s.to_string()).collect();
    expected.push("late.json".into());
    expected.sort();
    assert_eq!(files_under(&mailbox(dir.path())), expected);
    let retained = retained.expect("the retained copy is kept");
    assert_eq!(files_under(&retained.join("data")), SEEDED);
    assert_open(dir.path());
}

#[test]
fn a_write_into_a_listed_subdirectory_during_preserve_refuses_and_reopens() {
    let dir = repo();
    seed_mail(dir.path());
    activate(dir.path());
    // `.read` is listed and moved first; a writer that takes no permit adds
    // to it before it is removed.
    let extra = mailbox(dir.path()).join(".read/alice.txt");
    let extra_path = extra.clone();
    HOOK_AT.with(|h| {
        *h.borrow_mut() = Some((
            Point::MidPreserve,
            Box::new(move || std::fs::write(&extra_path, "3").unwrap()),
        ))
    });
    let (reason, retained) = refusal(retire_source(dir.path(), &opts()).unwrap());
    assert!(
        matches!(reason, RefusalReason::StateChanged { .. }),
        "{reason:?}"
    );
    assert!(retained.is_some());
    let mut expected: Vec<String> = SEEDED.iter().map(|s| s.to_string()).collect();
    expected.push(".read/alice.txt".into());
    expected.sort();
    assert_eq!(files_under(&mailbox(dir.path())), expected);
    assert_open(dir.path());
}

#[test]
fn a_permit_with_unknown_liveness_refuses() {
    let dir = repo();
    ensure_mailbox(dir.path());
    for holder in [
        None,
        Some(ProcessIdentity {
            pid: 1,
            start: "not-a-linux-identity".into(),
        }),
    ] {
        edit_record(dir.path(), |r| {
            r.permits = vec![Permit {
                permit_id: "p".into(),
                kind: PermitKind::Local,
                holder: holder.clone(),
                generation: r.generation,
            }]
        });
        let (reason, _) = refusal(retire_source(dir.path(), &opts()).unwrap());
        assert!(
            matches!(reason, RefusalReason::UnknownWriterLiveness { .. }),
            "{reason:?}"
        );
        assert_eq!(record(dir.path()).state, SourceState::Open);
    }
}

#[test]
fn a_permit_whose_writer_is_proven_dead_is_reaped() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let procfs = tempfile::tempdir().unwrap();
    let _fake = FakeProc::new(procfs.path(), "boot-a");
    edit_record(dir.path(), |r| {
        r.permits = vec![Permit {
            permit_id: "p".into(),
            kind: PermitKind::Local,
            holder: Some(ProcessIdentity {
                pid: 4242,
                start: "linux:boot-a:7".into(),
            }),
            generation: r.generation,
        }]
    });
    let opts = RetireOptions {
        probe: ProbeSource::at(procfs.path()),
        ..opts()
    };
    retired(retire_source(dir.path(), &opts).unwrap());
    assert!(record(dir.path()).permits.is_empty());
}

#[test]
fn a_live_writer_past_the_bounded_wait_refuses() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let permit = admit_local_write(dir.path()).unwrap().unwrap();
    let opts = RetireOptions {
        quiesce_timeout: Duration::from_millis(100),
        ..opts()
    };
    let (reason, _) = refusal(retire_source(dir.path(), &opts).unwrap());
    assert_eq!(reason, RefusalReason::LiveWriters { permits: 1 });
    permit.complete().unwrap();
    assert_open(dir.path());
}

#[test]
fn a_busy_lock_refuses_retirement_instead_of_blocking() {
    let dir = repo();
    let anchor = source(dir.path()).anchor;
    let (taken_tx, taken_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _held = lock(&anchor, LIFECYCLE_LOCK_TIMEOUT).unwrap();
        taken_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    taken_rx.recv().unwrap();
    let (reason, _) = refusal(retire_source(dir.path(), &opts()).unwrap());
    assert_eq!(reason, RefusalReason::LockBusy);
    release_tx.send(()).unwrap();
    holder.join().unwrap();
}

// ── Crash replay ────────────────────────────────────────────────────────────

fn crash_then_replay(at: Point) {
    let dir = repo();
    seed_mail(dir.path());
    activate(dir.path());
    CRASH_AT.with(|c| c.set(Some(at)));
    let crashed = retire_source(dir.path(), &opts());
    CRASH_AT.with(|c| c.set(None));
    assert!(
        matches!(crashed, Err(LifecycleError::InjectedCrash(_))),
        "{at:?}: {crashed:?}"
    );
    // Whatever survived the crash, no new admission gets in once the fence
    // is durable, and the next call replays forward to the same retirement.
    if at != Point::Journal(Phase::Fenced) {
        assert_retiring(dir.path());
    }
    let (ticket, retained) = retired(retire_source(dir.path(), &opts()).unwrap());
    assert_eq!(files_under(&retained.join("data")), SEEDED, "{at:?}");
    assert_eq!(files_under(&mailbox(dir.path())), Vec::<String>::new());
    assert_eq!(journal(&ticket).phase, Phase::Committed);
    let entries = std::fs::read_dir(ticket.anchor.join(JOURNAL_DIR))
        .unwrap()
        .count();
    assert_eq!(
        entries, 1,
        "{at:?}: replay must not start a second operation"
    );
    assert_retiring(dir.path());
}

#[test]
fn a_crash_after_each_journal_phase_replays_forward() {
    for phase in [
        Phase::Fenced,
        Phase::Quiesced,
        Phase::Preserved,
        Phase::Committed,
    ] {
        crash_then_replay(Point::Journal(phase));
    }
}

#[test]
fn a_crash_after_the_fence_or_mid_preserve_replays_forward() {
    crash_then_replay(Point::AfterFence);
    crash_then_replay(Point::MidPreserve);
    crash_then_replay(Point::BeforeFinalCheck);
}

#[test]
fn a_crash_after_removed_finishes_on_the_next_completion() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let (ticket, _) = retired(retire_source(dir.path(), &opts()).unwrap());
    let key = journal(&ticket).key;
    std::fs::remove_dir_all(dir.path().join(".aida")).unwrap();
    CRASH_AT.with(|c| c.set(Some(Point::Journal(Phase::Removed))));
    let crashed = complete_retirement(&ticket);
    CRASH_AT.with(|c| c.set(None));
    assert!(matches!(crashed, Err(LifecycleError::InjectedCrash(_))));
    assert_eq!(journal(&ticket).phase, Phase::Removed);
    assert!(record_path(&ticket.anchor, &key).exists());
    complete_retirement(&ticket).unwrap();
    assert_eq!(journal(&ticket).phase, Phase::Finalized);
    assert!(!record_path(&ticket.anchor, &key).exists());
}

#[test]
fn a_crash_after_aborted_reopens_on_the_next_call() {
    let dir = repo();
    seed_mail(dir.path());
    CRASH_AT.with(|c| c.set(Some(Point::Journal(Phase::Aborted))));
    let crashed = retire_source(dir.path(), &opts());
    CRASH_AT.with(|c| c.set(None));
    assert!(matches!(crashed, Err(LifecycleError::InjectedCrash(_))));
    assert_retiring(dir.path());
    let (reason, _) = refusal(retire_source(dir.path(), &opts()).unwrap());
    assert_eq!(reason, RefusalReason::MailBearingBeforeActivation);
    assert_eq!(files_under(&mailbox(dir.path())), SEEDED);
    assert_open(dir.path());
}

#[test]
fn a_crash_after_a_late_writer_abort_restores_on_replay() {
    let dir = repo();
    seed_mail(dir.path());
    activate(dir.path());
    let late = mailbox(dir.path()).join("late.json");
    HOOK_AT.with(|h| {
        *h.borrow_mut() = Some((
            Point::BeforeFinalCheck,
            Box::new(move || std::fs::write(&late, "{}").unwrap()),
        ))
    });
    CRASH_AT.with(|c| c.set(Some(Point::Journal(Phase::Aborted))));
    let crashed = retire_source(dir.path(), &opts());
    CRASH_AT.with(|c| c.set(None));
    assert!(matches!(crashed, Err(LifecycleError::InjectedCrash(_))));
    // The preserved bytes are out of the tree until the abort is replayed.
    assert_eq!(files_under(&mailbox(dir.path())), vec!["late.json"]);
    // The next call restores them, reopens, and retires afresh: the late
    // file is now part of the source and preserved too.
    let (_, retained) = retired(retire_source(dir.path(), &opts()).unwrap());
    let mut expected: Vec<String> = SEEDED.iter().map(|s| s.to_string()).collect();
    expected.push("late.json".into());
    expected.sort();
    assert_eq!(files_under(&retained.join("data")), expected);
}

#[test]
fn generations_are_minted_monotonically_across_a_reused_source() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let first = record(dir.path()).generation;
    let (ticket, _) = retired(retire_source(dir.path(), &opts()).unwrap());
    std::fs::remove_dir_all(dir.path().join(".aida")).unwrap();
    complete_retirement(&ticket).unwrap();
    write_local_message(dir.path(), &message("next")).unwrap();
    let next = record(dir.path());
    assert!(next.generation > first);
    assert_eq!(next.state, SourceState::Open);
}

//! C3x fixtures: the canonical publication permit and the guarded store
//! worktree removal (C SKETCH v1 sections 3 and 6).
//! trace:STORY-1843 | ai:claude

use super::retire::{retire_source, RefusalReason, RetireOptions};
use super::tests::{
    dead_identity, edit_record, ensure_mailbox, opts, record, refusal, repo, retired, source,
};
use super::*;

fn canonical_rows(root: &Path) -> Vec<Permit> {
    record(root)
        .permits
        .into_iter()
        .filter(|p| p.kind == PermitKind::Canonical)
        .collect()
}

/// The source record read without the Lifecycle lock, as code inside a
/// Store-held region may.
fn record_unlocked(root: &Path) -> SourceRecord {
    let source = source(root);
    read_json(&record_path(&source.anchor, &source.key))
        .unwrap()
        .unwrap()
}

fn lifecycle_error(e: &std::io::Error) -> Option<&LifecycleError> {
    e.get_ref().and_then(|e| e.downcast_ref::<LifecycleError>())
}

#[test]
fn a_canonical_permit_is_a_generation_bound_row_released_after_the_store_lock() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let store = tempfile::tempdir().unwrap();
    let permit = admit_canonical_publication(dir.path()).unwrap();
    assert!(permit.is_admitted());
    let rows = canonical_rows(dir.path());
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].generation, record(dir.path()).generation);
    assert!(rows[0].holder.is_some(), "the row names this process");

    let root = dir.path().to_path_buf();
    let ((), classes) = observe_lock_classes(|| {
        crate::db::with_store_write_lock(store.path(), move || {
            let permit = permit;
            assert!(permit.is_admitted());
            drop(permit);
            // Dropped inside the Store-held region: the row stays until the
            // outermost Store guard is released.
            assert_eq!(record_unlocked(&root).permits.len(), 1);
            Ok(())
        })
        .unwrap()
    });
    assert!(!classes.contains(&UNDER_LIFECYCLE), "{classes:?}");
    assert!(
        canonical_rows(dir.path()).is_empty(),
        "released after unlock"
    );
}

#[test]
fn a_permit_dropped_outside_any_store_lock_releases_at_once() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let permit = admit_canonical_publication(dir.path()).unwrap();
    assert_eq!(canonical_rows(dir.path()).len(), 1);
    drop(permit);
    assert!(canonical_rows(dir.path()).is_empty());
}

#[test]
fn a_nested_store_region_defers_release_to_the_outermost_guard() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let store = tempfile::tempdir().unwrap();
    let permit = admit_canonical_publication(dir.path()).unwrap();
    let root = dir.path().to_path_buf();
    crate::db::with_store_write_lock(store.path(), || {
        crate::db::with_store_write_lock(store.path(), move || {
            drop(permit);
            Ok(())
        })?;
        // The re-entrant inner guard released one level only.
        assert_eq!(record_unlocked(&root).permits.len(), 1);
        Ok(())
    })
    .unwrap();
    assert!(canonical_rows(dir.path()).is_empty());
}

#[test]
fn admission_under_a_store_lock_refuses_as_a_lock_order_violation() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let store = tempfile::tempdir().unwrap();
    let refused = crate::db::with_store_write_lock(store.path(), || {
        Ok(admit_canonical_publication(dir.path()).unwrap_err())
    })
    .unwrap();
    assert!(
        matches!(
            lifecycle_error(&refused),
            Some(LifecycleError::LockOrder(_))
        ),
        "{refused}"
    );
    assert!(record(dir.path()).permits.is_empty());
}

#[test]
fn a_fenced_source_refuses_canonical_admission() {
    let dir = repo();
    ensure_mailbox(dir.path());
    edit_record(dir.path(), |r| {
        r.state = SourceState::Fenced {
            op_id: "op".into(),
            reason: "test".into(),
        }
    });
    let refused = admit_canonical_publication(dir.path()).unwrap_err();
    assert!(
        matches!(
            lifecycle_error(&refused),
            Some(LifecycleError::SourceRetiring { .. })
        ),
        "{refused}"
    );
}

#[test]
fn no_local_mailbox_or_no_git_admits_uncovered() {
    let dir = repo();
    assert!(!admit_canonical_publication(dir.path())
        .unwrap()
        .is_admitted());
    let plain = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(crate::mailbox::local_mailbox_dir(plain.path())).unwrap();
    assert!(!admit_canonical_publication(plain.path())
        .unwrap()
        .is_admitted());
}

#[test]
fn a_live_canonical_permit_keeps_retirement_waiting_then_refusing() {
    let dir = repo();
    ensure_mailbox(dir.path());
    let permit = admit_canonical_publication(dir.path()).unwrap();
    let short = RetireOptions {
        quiesce_timeout: Duration::from_millis(100),
        ..opts()
    };
    let (reason, _) = refusal(retire_source(dir.path(), &short).unwrap());
    assert_eq!(reason, RefusalReason::LiveWriters { permits: 1 });

    // A retirement that starts while a long publication is still running
    // waits for it and then completes.
    let root = dir.path().to_path_buf();
    let retirer = std::thread::spawn(move || {
        let patient = RetireOptions {
            quiesce_timeout: Duration::from_secs(20),
            ..RetireOptions::default()
        };
        retire_source(&root, &patient)
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !matches!(record(dir.path()).state, SourceState::Fenced { .. }) {
        assert!(Instant::now() < deadline, "the retirer never fenced");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(permit);
    retired(retirer.join().unwrap().unwrap());
}

#[test]
fn live_publications_count_canonical_rows_unless_proven_dead() {
    let dir = repo();
    ensure_mailbox(dir.path());
    assert_eq!(live_canonical_publications(dir.path()).unwrap(), 0);
    let local = admit_local_write(dir.path()).unwrap().unwrap();
    assert_eq!(live_canonical_publications(dir.path()).unwrap(), 0);
    let permit = admit_canonical_publication(dir.path()).unwrap();
    assert_eq!(live_canonical_publications(dir.path()).unwrap(), 1);
    drop(permit);
    local.complete().unwrap();

    let dead = dead_identity();
    edit_record(dir.path(), |r| {
        r.permits = vec![
            Permit {
                permit_id: "dead".into(),
                kind: PermitKind::Canonical,
                holder: Some(dead.clone()),
                generation: r.generation,
            },
            Permit {
                permit_id: "unknown".into(),
                kind: PermitKind::Canonical,
                holder: None,
                generation: r.generation,
            },
        ]
    });
    assert_eq!(live_canonical_publications(dir.path()).unwrap(), 1);
}

// ── remove_store_worktree (QC4) ─────────────────────────────────────────────

fn git_in(cwd: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A project repository with a store worktree at `.aida-store` whose
/// `mailbox/` holds one committed message.
fn project_with_store() -> (tempfile::TempDir, PathBuf) {
    let dir = repo();
    git_in(dir.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
    git_in(
        dir.path(),
        &["worktree", "add", "-q", "-b", "aida-store", ".aida-store"],
    );
    let store = dir.path().join(".aida-store");
    std::fs::create_dir_all(store.join("mailbox")).unwrap();
    std::fs::write(store.join("mailbox/m1.json"), "{}").unwrap();
    git_in(&store, &["add", "mailbox"]);
    git_in(&store, &["commit", "-q", "-m", "mail"]);
    (dir, store)
}

fn remove(dir: &Path) -> anyhow::Result<()> {
    crate::git_ops::remove_store_worktree(dir, ".aida-store")
}

#[test]
fn a_clean_store_worktree_is_removed() {
    let (dir, store) = project_with_store();
    remove(dir.path()).unwrap();
    assert!(!store.exists());
}

#[test]
fn untracked_canonical_mail_keeps_the_store_worktree() {
    let (dir, store) = project_with_store();
    std::fs::write(store.join("mailbox/m2.json"), "{}").unwrap();
    let err = remove(dir.path()).unwrap_err().to_string();
    assert!(err.contains("not committed"), "{err}");
    assert!(err.contains("mailbox/m2.json"), "{err}");
    assert!(store.join("mailbox/m2.json").is_file());
}

#[test]
fn uncommitted_canonical_mail_keeps_the_store_worktree() {
    let (dir, store) = project_with_store();
    std::fs::write(store.join("mailbox/m1.json"), "{\"changed\":1}").unwrap();
    let err = remove(dir.path()).unwrap_err().to_string();
    assert!(err.contains("mailbox/m1.json"), "{err}");
    assert!(store.join("mailbox/m1.json").is_file());
}

#[test]
fn a_live_canonical_publication_keeps_the_store_worktree() {
    let (dir, store) = project_with_store();
    ensure_mailbox(dir.path());
    let permit = admit_canonical_publication(dir.path()).unwrap();
    assert!(permit.is_admitted());
    let err = remove(dir.path()).unwrap_err().to_string();
    assert!(err.contains("still writing"), "{err}");
    assert!(store.join("mailbox/m1.json").is_file());
    drop(permit);
    remove(dir.path()).unwrap();
    assert!(!store.exists());
}

#[test]
fn files_outside_the_mailbox_do_not_block_removal() {
    let (dir, store) = project_with_store();
    std::fs::write(store.join("scratch.txt"), "x").unwrap();
    remove(dir.path()).unwrap();
    assert!(!store.exists());
}

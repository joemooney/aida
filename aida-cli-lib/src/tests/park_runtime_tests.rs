// trace:STORY-2661 | ai:codex
use super::*;

fn fixture() -> (Uuid, DecisionRequest, ParkRecord) {
    let mut question: DecisionRequest = serde_json::from_value(serde_json::json!({
        "question": "Which name?",
        "choices": [
            {"label":"One", "consequence":"Use one", "resolution":"one"},
            {"label":"Two", "consequence":"Use two", "resolution":"two"}
        ]
    }))
    .unwrap();
    let question_id = migrate_question_identity(&mut question).unwrap();
    let binding = ParkBinding {
        version: 1,
        wait_id: Uuid::new_v4(),
        generation: 1,
        owner_node_id: "host-one".into(),
    };
    question.park_binding = Some(binding.clone());
    let spec = Uuid::new_v4();
    let now = chrono::Utc::now();
    let record = ParkRecord {
        version: 1,
        binding,
        spec_uuid: spec,
        question_id,
        question_hash: question_hash(&question).unwrap(),
        owner_role: "research".into(),
        lease_id: "lease".into(),
        run_id: "run".into(),
        phase: "implement".into(),
        vendor: "fixture".into(),
        vendor_session_id: "exact-session".into(),
        capture_source: "host".into(),
        launch_generation: 1,
        harness_version: None,
        seat_path: "/retained/dirty-seat".into(),
        worktree_identity: "identity".into(),
        branch: "work".into(),
        head_sha: "head".into(),
        posture: "native".into(),
        grant_reference: "reference-only".into(),
        requester_identity: "requester".into(),
        state: WaitState::ParkRequested,
        created_at: now,
        updated_at: now,
        answer_hash: None,
        answer_event: None,
        recovery_reason: None,
    };
    (spec, question, record)
}

#[test]
fn legacy_question_migration_is_stable_and_preserves_answer() {
    let (_, mut question, _) = fixture();
    question.park_binding = None;
    question.question_id = None;
    question.answered = Some(0);
    question.note = Some("1, but name it X".into());
    let before = question_hash(&question).unwrap();
    let id = migrate_question_identity(&mut question).unwrap();
    let persisted = serde_json::to_vec(&question).unwrap();
    let mut restarted: DecisionRequest = serde_json::from_slice(&persisted).unwrap();
    assert_eq!(migrate_question_identity(&mut restarted).unwrap(), id);
    assert_eq!(restarted.note.as_deref(), Some("1, but name it X"));
    assert_eq!(question_hash(&restarted).unwrap(), before);
    restarted.park_binding = fixture().1.park_binding;
    restarted.question_id = None;
    assert!(migrate_question_identity(&mut restarted).is_err());
}

#[test]
fn restart_at_each_binding_and_record_write_boundary_retains_work() {
    let (spec, question, record) = fixture();
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("waits");
    let id = record.binding.wait_id;
    // Before canonical write: ordinary unbound question.
    assert_eq!(reconcile(spec, None, None), Ownership::Unbound);
    // Canonical commit persisted, host record not written yet.
    let canonical = serde_json::to_vec(&question).unwrap();
    let restarted: DecisionRequest = serde_json::from_slice(&canonical).unwrap();
    let lock = WaitLock::acquire(&runtime, id).unwrap();
    assert!(matches!(
        reconcile(spec, Some(&restarted), lock.read().unwrap().as_ref()),
        Ownership::RecoveryRequired(_)
    ));
    lock.write(&record).unwrap();
    drop(lock);
    // Both writes persisted, receipt/exit handshake deliberately absent.
    let lock = WaitLock::acquire(&runtime, id).unwrap();
    let restored = lock.read().unwrap().unwrap();
    assert_eq!(restored, record);
    assert_eq!(
        reconcile(spec, Some(&restarted), Some(&restored)),
        Ownership::Retained
    );
    // Runtime-only evidence after canonical deletion must not release the seat.
    assert!(matches!(
        reconcile(spec, None, Some(&restored)),
        Ownership::RecoveryRequired(_)
    ));
    assert!(WaitLock::acquire(&runtime, id).is_err());
}

#[test]
fn answers_and_blocked_states_do_not_release_ownership() {
    let (spec, mut question, mut record) = fixture();
    question.answered = Some(0);
    question.note = Some("1, but name it X".into());
    question.answered_at = Some(chrono::Utc::now());
    for state in [
        WaitState::Parked,
        WaitState::ResumeReady,
        WaitState::Resuming,
        WaitState::BlockedAuthority,
        WaitState::DeliveryUnknown,
        WaitState::PostExitRecoveryRequired,
        WaitState::RecoveryRequired,
    ] {
        record.state = state;
        assert_eq!(
            reconcile(spec, Some(&question), Some(&record)),
            Ownership::Retained
        );
    }
    record.answer_hash = answer_hash(&question).unwrap();
    question.note = Some("changed answer".into());
    assert!(matches!(
        reconcile(spec, Some(&question), Some(&record)),
        Ownership::RecoveryRequired(_)
    ));
    record.answer_hash = answer_hash(&question).unwrap();
    record.state = WaitState::Released;
    assert_eq!(
        reconcile(spec, Some(&question), Some(&record)),
        Ownership::Released
    );
    question.park_binding.as_mut().unwrap().generation += 1;
    assert!(matches!(
        reconcile(spec, Some(&question), Some(&record)),
        Ownership::RecoveryRequired(_)
    ));
}

#[test]
fn unsafe_or_corrupt_runtime_evidence_refuses_read_and_overwrite() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (_, _, record) = fixture();
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("waits");
    let lock = WaitLock::acquire(&runtime, record.binding.wait_id).unwrap();
    let path = runtime.join(format!("{}.json", record.binding.wait_id));
    let outside = dir.path().join("outside");
    std::fs::write(&outside, b"untouched").unwrap();
    symlink(&outside, &path).unwrap();
    assert!(lock.read().is_err());
    assert!(lock.write(&record).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"{").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(lock.read().is_err());
    assert!(lock.write(&record).is_err());
    assert_eq!(std::fs::read(&outside).unwrap(), b"untouched");
}

#[test]
fn runtime_directory_creation_syncs_pinned_parent_before_lock_creation() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("waits");
    let id = Uuid::new_v4();
    let expected_parent = dir.path().metadata().unwrap();
    let mut synced = false;
    let result = WaitLock::acquire_with_parent_sync(&runtime, id, |parent| {
        synced = true;
        let actual_parent = parent.metadata()?;
        assert_eq!(actual_parent.dev(), expected_parent.dev());
        assert_eq!(actual_parent.ino(), expected_parent.ino());
        assert!(runtime.is_dir());
        assert_eq!(std::fs::read_dir(&runtime)?.count(), 0);
        Err(std::io::Error::other("injected parent sync failure"))
    });
    assert!(synced);
    let error = result
        .err()
        .expect("parent sync failure must refuse acquisition");
    assert_eq!(error.to_string(), "injected parent sync failure");
    assert_eq!(std::fs::read_dir(&runtime).unwrap().count(), 0);

    // Retry must make the entry durable even though the directory now exists.
    let mut retry_synced = false;
    let lock = WaitLock::acquire_with_parent_sync(&runtime, id, |parent| {
        retry_synced = true;
        let actual_parent = parent.metadata()?;
        assert_eq!(actual_parent.dev(), expected_parent.dev());
        assert_eq!(actual_parent.ino(), expected_parent.ino());
        assert_eq!(std::fs::read_dir(&runtime)?.count(), 0);
        parent.sync_all()
    })
    .unwrap();
    assert!(retry_synced);
    assert!(lock.read().unwrap().is_none());
}

#[test]
fn existing_runtime_directory_syncs_pinned_parent_before_lock_creation() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("waits");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&runtime)
        .unwrap();
    let expected_parent = dir.path().metadata().unwrap();
    let mut synced = false;
    let lock = WaitLock::acquire_with_parent_sync(&runtime, Uuid::new_v4(), |parent| {
        synced = true;
        let actual_parent = parent.metadata()?;
        assert_eq!(actual_parent.dev(), expected_parent.dev());
        assert_eq!(actual_parent.ino(), expected_parent.ino());
        assert_eq!(std::fs::read_dir(&runtime)?.count(), 0);
        parent.sync_all()
    })
    .unwrap();
    assert!(synced);
    assert!(lock.read().unwrap().is_none());
}

#[test]
fn atomic_replacement_recovers_without_consuming_abandoned_temporary_files() {
    let (spec, question, mut record) = fixture();
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("waits");
    let id = record.binding.wait_id;
    let lock = WaitLock::acquire(&runtime, id).unwrap();
    lock.write(&record).unwrap();
    // Crash during a later temporary write leaves the last complete record.
    std::fs::write(runtime.join("abandoned.tmp"), b"{partial").unwrap();
    drop(lock);
    let lock = WaitLock::acquire(&runtime, id).unwrap();
    assert_eq!(lock.read().unwrap(), Some(record.clone()));
    record.state = WaitState::Parked;
    lock.write(&record).unwrap();
    drop(lock);
    let lock = WaitLock::acquire(&runtime, id).unwrap();
    assert_eq!(
        reconcile(spec, Some(&question), lock.read().unwrap().as_ref()),
        Ownership::Retained
    );
    let mut replacement = record.clone();
    replacement.spec_uuid = Uuid::new_v4();
    assert!(lock.write(&replacement).is_err());
    record.state = WaitState::Released;
    lock.write(&record).unwrap();
    record.state = WaitState::Parked;
    assert!(lock.write(&record).is_err());
}

#[test]
fn bounded_regular_file_reads_reject_hardlinks_fifo_and_oversize() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    let (_, _, record) = fixture();
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("waits");
    let lock = WaitLock::acquire(&runtime, record.binding.wait_id).unwrap();
    lock.write(&record).unwrap();
    let path = runtime.join(format!("{}.json", record.binding.wait_id));
    let link = dir.path().join("hardlink");
    std::fs::hard_link(&path, &link).unwrap();
    assert!(lock.read().is_err());
    std::fs::remove_file(&link).unwrap();
    std::fs::remove_file(&path).unwrap();
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(lock.read().is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, vec![b' '; LIMIT as usize + 1]).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(lock.read().is_err());
}

#[test]
fn legacy_migration_persists_through_canonical_writer_and_restart() {
    use aida_core::DatabaseBackend;
    let dir = tempfile::tempdir().unwrap();
    let store_path = dir.path().join(".aida-store");
    std::fs::create_dir(&store_path).unwrap();
    let storage = crate::test_env::home_fixture_storage(&store_path);
    let mut req = aida_core::Requirement::new("Legacy question".into(), String::new());
    req.spec_id = Some("STORY-9944".into());
    let (_, mut question, _) = fixture();
    question.question_id = None;
    question.park_binding = None;
    req.decision_request = Some(question);
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements.push(req.clone());
    storage.save(&store).unwrap();
    let cache = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::open(&store_path, &cache).unwrap();
    let lock = WaitLock::acquire(&dir.path().join("waits"), Uuid::new_v4()).unwrap();
    let migrated = lock.migrate_legacy_question(&backend, &req).unwrap();
    assert!(migrated.question_id.is_some());
    drop(backend);
    let backend = aida_core::CachedGitBackend::open(&store_path, &cache).unwrap();
    let persisted = backend
        .get_requirement_unambiguous("STORY-9944")
        .unwrap()
        .unwrap();
    assert_eq!(persisted.decision_request.as_ref().unwrap(), &migrated);
    assert_eq!(
        lock.migrate_legacy_question(&backend, &persisted).unwrap(),
        migrated
    );
}

#[test]
fn symlinked_runtime_parent_is_refused_before_directory_creation() {
    let dir = tempfile::tempdir().unwrap();
    let actual = dir.path().join("actual");
    std::fs::create_dir(&actual).unwrap();
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(&actual, &alias).unwrap();
    assert!(WaitLock::acquire(&alias.join("waits"), Uuid::new_v4()).is_err());
    assert!(!actual.join("waits").exists());
}

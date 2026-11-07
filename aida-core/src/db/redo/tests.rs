// trace:TASK-3223 | ai:codex
use super::*;
use crate::db::{canonical_mutation::CanonicalOperation, with_store_write_lock, GitBackend};

fn fixture(root: &Path) -> (Decision, CanonicalOperation) {
    crate::allocation_authority::home_fixture_backend(root).unwrap();
    fs::create_dir_all(root.join("objects")).unwrap();
    let mut row = crate::Requirement::new("before".into(), "description".into());
    row.spec_id = Some("TASK-1".into());
    crate::object_store::write_object(&root.join("objects"), &row).unwrap();
    let bytes =
        fs::read(crate::object_store::object_path(&root.join("objects"), "TASK-1").unwrap())
            .unwrap();
    let op = CanonicalOperation::CommentAdd {
        spec_id: "TASK-1".into(),
        expected_revision: hash(&bytes),
        content: "one comment".into(),
    };
    row.add_comment(crate::models::Comment::new(
        "bound author".into(),
        "one comment".into(),
    ));
    let mut other = crate::Requirement::new("other".into(), "description".into());
    other.spec_id = Some("TASK-2".into());
    crate::object_store::write_object(&root.join("objects"), &other).unwrap();
    let bytes2 =
        fs::read(crate::object_store::object_path(&root.join("objects"), "TASK-2").unwrap())
            .unwrap();
    other.title = "after".into();
    (
        Decision {
            version: 1,
            binding: "fixed".into(),
            epoch: uuid::Uuid::new_v4(),
            request_id: uuid::Uuid::new_v4(),
            digest: hash(&serde_json::to_vec(&op).unwrap()),
            images: vec![
                Image {
                    spec_id: "TASK-1".into(),
                    before: hash(&bytes),
                    after: serde_yaml::to_string(&row).unwrap(),
                },
                Image {
                    spec_id: "TASK-2".into(),
                    before: hash(&bytes2),
                    after: serde_yaml::to_string(&other).unwrap(),
                },
            ],
            result: serde_json::json!({"comment_id":row.comments[0].id,"exact":"durable reply"}),
        },
        op,
    )
}
fn child(root: &Path, checkpoint: &str) -> std::process::ExitStatus {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "db::redo::tests::restart_child", "--nocapture"])
        .env("AIDA_TEST_REDO_ROOT", root)
        .env("AIDA_TEST_REDO_CHECKPOINT", checkpoint);
    let mut child = command.spawn().unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if std::time::Instant::now() >= until {
            let _ = child.kill();
            panic!("redo child watchdog");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
#[test]
fn restart_child() {
    let Some(root) = std::env::var_os("AIDA_TEST_REDO_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let checkpoint = std::env::var("AIDA_TEST_REDO_CHECKPOINT").unwrap();
    if checkpoint == "recover" {
        with_store_write_lock(&root, || {
            let row = crate::object_store::read_object(&root.join("objects"), "TASK-1")?;
            assert_eq!(row.comments.len(), 1);
            assert_eq!(
                crate::object_store::read_object(&root.join("objects"), "TASK-2")?.title,
                "after"
            );
            Ok(())
        })
        .unwrap();
    } else {
        let d: Decision =
            serde_json::from_slice(&fs::read(root.join("decision.json")).unwrap()).unwrap();
        let mut installs = 0;
        with_store_write_lock(&root, || {
            commit_with_fault(&root, d, &mut |point| {
                if point == "install" {
                    installs += 1;
                }
                if checkpoint == point
                    || checkpoint == format!("install-{installs}") && point == "install"
                {
                    std::process::exit(72);
                }
                Ok(())
            })
        })
        .unwrap();
        panic!("fault was not reached");
    }
}
#[test]
fn restart_recovers_each_boundary_and_replays_exact_result_once() {
    for point in ["journal", "install-1", "install-2", "durable", "reply"] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let (d, op) = fixture(root);
        fs::write(root.join("decision.json"), serde_json::to_vec(&d).unwrap()).unwrap();
        assert_eq!(child(root, point).code(), Some(72), "{point}");
        assert!(child(root, "recover").success(), "{point}");
        // A retry uses stale preimage input but returns the identical durable
        // result; no business logic, timestamp or comment ID is regenerated.
        let backend = GitBackend::new(root).unwrap();
        assert_eq!(
            backend
                .mutate_canonical("fixed", d.epoch, d.request_id, "bound author", &op, || Ok(
                    ()
                ))
                .unwrap(),
            d.result
        );
        assert!(backend
            .mutate_canonical(
                "fixed",
                d.epoch,
                d.request_id,
                "bound author",
                &op,
                || anyhow::bail!("Revoked")
            )
            .is_err());
        assert_eq!(
            crate::object_store::read_object(&root.join("objects"), "TASK-1")
                .unwrap()
                .comments
                .len(),
            1
        );
        let mut changed = op.clone();
        if let CanonicalOperation::CommentAdd { content, .. } = &mut changed {
            *content = "changed".into();
        }
        assert!(backend
            .mutate_canonical(
                "fixed",
                d.epoch,
                d.request_id,
                "bound author",
                &changed,
                || Ok(())
            )
            .unwrap_err()
            .to_string()
            .contains("Conflict"));
    }
}
#[test]
fn unexpected_bytes_quarantine_and_fence_all_writers() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (d, _) = fixture(root);
    fs::write(root.join("decision.json"), serde_json::to_vec(&d).unwrap()).unwrap();
    assert_eq!(child(root, "journal").code(), Some(72));
    let path = crate::object_store::object_path(&root.join("objects"), "TASK-2").unwrap();
    fs::write(&path, "unexpected").unwrap();
    for _ in 0..2 {
        let e = with_store_write_lock(root, || -> Result<()> {
            panic!("writer admitted");
        })
        .unwrap_err();
        assert!(e.downcast_ref::<OutcomeUnknown>().is_some(), "{e:#}");
    }
    assert_eq!(
        crate::object_store::read_object(&root.join("objects"), "TASK-1")
            .unwrap()
            .comments
            .len(),
        0
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "unexpected");
}
#[test]
fn corrupt_decision_and_unknown_format_refuse_before_writer() {
    for unknown_version in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dir = journal(root).unwrap();
        if unknown_version {
            fs::write(dir.join("format"), "99\n").unwrap();
        } else {
            fs::write(dir.join("bad.pending"), "{broken").unwrap();
        }
        assert!(with_store_write_lock(root, || -> Result<()> {
            panic!("writer admitted");
        })
        .is_err());
        assert!(!super::super::store_write_lock_held(root));
    }
}
#[test]
fn canonical_comment_edit_stale_and_concurrent_requests() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_, op) = fixture(root);
    let epoch = uuid::Uuid::new_v4();
    let id = uuid::Uuid::new_v4();
    let mut threads = Vec::new();
    for _ in 0..3 {
        let root = root.to_path_buf();
        let op = op.clone();
        threads.push(std::thread::spawn(move || {
            GitBackend::new(&root)
                .unwrap()
                .mutate_canonical("fixed", epoch, id, "author", &op, || Ok(()))
                .unwrap()
        }));
    }
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert!(results.iter().all(|r| r == &results[0]));
    let backend = GitBackend::new(root).unwrap();
    assert!(backend
        .mutate_canonical("fixed", epoch, uuid::Uuid::new_v4(), "author", &op, || Ok(
            ()
        ))
        .unwrap_err()
        .to_string()
        .contains("Conflict"));
    let edit = CanonicalOperation::SpecEdit {
        spec_id: "TASK-1".into(),
        expected_revision: results[0]["revision"].as_str().unwrap().into(),
        fields: super::super::canonical_mutation::EditFields {
            title: Some("updated".into()),
            description: None,
        },
    };
    backend
        .mutate_canonical(
            "fixed",
            epoch,
            uuid::Uuid::new_v4(),
            "author",
            &edit,
            || Ok(()),
        )
        .unwrap();
    let row = crate::object_store::read_object(&root.join("objects"), "TASK-1").unwrap();
    assert_eq!(row.title, "updated");
    assert_eq!(row.comments.len(), 1);
    assert_eq!(row.comments[0].author, "author");
    assert!(row.history.iter().any(|h| h.author == "author"));
}

#[test]
fn io_failure_never_replies_success_and_next_writer_recovers() {
    for point in ["journal", "install", "durable", "reply"] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let (d, _) = fixture(root);
        let result = with_store_write_lock(root, || {
            commit_with_fault(root, d.clone(), &mut |p| {
                if p == point {
                    anyhow::bail!("injected I/O failure");
                }
                Ok(())
            })
        });
        assert!(result
            .unwrap_err()
            .downcast_ref::<OutcomeUnknown>()
            .is_some());
        with_store_write_lock(root, || {
            assert_eq!(
                lookup(root, &d.binding, d.epoch, d.request_id, &d.digest)?.unwrap(),
                d.result
            );
            assert_eq!(
                crate::object_store::read_object(&root.join("objects"), "TASK-1")?
                    .comments
                    .len(),
                1
            );
            Ok(())
        })
        .unwrap();
    }
}
#[test]
fn competing_requests_cannot_both_use_the_same_revision() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_, op) = fixture(root);
    let mut threads = Vec::new();
    for _ in 0..2 {
        let root = root.to_path_buf();
        let op = op.clone();
        threads.push(std::thread::spawn(move || {
            GitBackend::new(&root).unwrap().mutate_canonical(
                "fixed",
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
                "author",
                &op,
                || Ok(()),
            )
        }));
    }
    let outcomes: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        crate::object_store::read_object(&root.join("objects"), "TASK-1")
            .unwrap()
            .comments
            .len(),
        1
    );
}
#[test]
fn bounded_admission_refuses_without_entering_the_operation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let guard = super::super::store_lock::acquire(&root).unwrap();
    let worker = std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let result = super::super::with_store_write_lock_bounded(&root, || -> Result<()> {
            panic!("operation admitted");
        });
        assert!(result.is_err());
        assert!(start.elapsed() >= std::time::Duration::from_secs(5));
        assert!(start.elapsed() < std::time::Duration::from_secs(8));
    });
    worker.join().unwrap();
    drop(guard);
}
#[test]
fn cache_refreshes_after_canonical_install_without_a_git_commit() {
    use crate::db::DatabaseBackend;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_, op) = fixture(root);
    let cached =
        crate::CachedGitBackend::with_inner(GitBackend::new(root).unwrap(), &root.join("cache.db"))
            .unwrap();
    assert!(!cached.cache_snapshot_is_stale().unwrap());
    assert_eq!(
        cached
            .load()
            .unwrap()
            .requirements
            .iter()
            .find(|r| r.spec_id.as_deref() == Some("TASK-1"))
            .unwrap()
            .comments
            .len(),
        0
    );
    GitBackend::new(root)
        .unwrap()
        .mutate_canonical(
            "fixed",
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "author",
            &op,
            || Ok(()),
        )
        .unwrap();
    assert!(cached.cache_snapshot_is_stale().unwrap());
    assert_eq!(
        cached
            .load()
            .unwrap()
            .requirements
            .iter()
            .find(|r| r.spec_id.as_deref() == Some("TASK-1"))
            .unwrap()
            .comments
            .len(),
        1
    );
    assert!(!cached.cache_snapshot_is_stale().unwrap());
}
#[test]
fn locked_hypothesis_description_cannot_be_edited() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture(root);
    let mut row = crate::object_store::read_object(&root.join("objects"), "TASK-1").unwrap();
    row.hypothesis_lock = Some(crate::models::HypothesisLock {
        locked_at: chrono::Utc::now(),
        description_sha256: hash(row.description.as_bytes()),
    });
    crate::object_store::write_object(&root.join("objects"), &row).unwrap();
    let path = crate::object_store::object_path(&root.join("objects"), "TASK-1").unwrap();
    let before = fs::read(&path).unwrap();
    let op = CanonicalOperation::SpecEdit {
        spec_id: "TASK-1".into(),
        expected_revision: hash(&before),
        fields: super::super::canonical_mutation::EditFields {
            title: None,
            description: Some("rewritten hypothesis".into()),
        },
    };
    assert!(GitBackend::new(root)
        .unwrap()
        .mutate_canonical(
            "fixed",
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "author",
            &op,
            || Ok(())
        )
        .is_err());
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn full_journal_refuses_new_decisions_but_keeps_retry_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (d, _) = fixture(root);
    with_store_write_lock(root, || {
        commit(root, d.clone())?;
        Ok(())
    })
    .unwrap();
    let dir = root.join(".aida/redo");
    for n in 1..4096 {
        fs::write(dir.join(format!("retained-{n}.done")), b"retained evidence").unwrap();
    }
    with_store_write_lock(root, || {
        let mut next = d.clone();
        next.request_id = uuid::Uuid::new_v4();
        assert_eq!(commit(root, next).unwrap_err().to_string(), "Busy");
        assert_eq!(commit(root, d.clone())?, d.result);
        assert!(dir.join("retained-4095.done").exists());
        Ok(())
    })
    .unwrap();
}
#[test]
fn symlinked_canonical_destination_quarantines_without_touching_target() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (d, _) = fixture(root);
    fs::write(root.join("decision.json"), serde_json::to_vec(&d).unwrap()).unwrap();
    assert_eq!(child(root, "journal").code(), Some(72));
    let path = crate::object_store::object_path(&root.join("objects"), "TASK-1").unwrap();
    let outside = root.join("outside");
    fs::rename(&path, &outside).unwrap();
    let before = fs::read(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    assert!(with_store_write_lock(root, || -> Result<()> {
        panic!("writer admitted");
    })
    .unwrap_err()
    .downcast_ref::<OutcomeUnknown>()
    .is_some());
    assert_eq!(fs::read(outside).unwrap(), before);
}

#[test]
fn revocation_during_planning_refuses_before_durable_acceptance() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (_, op) = fixture(root);
    let mut checks = 0;
    let error = GitBackend::new(root)
        .unwrap()
        .mutate_canonical(
            "fixed",
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            "author",
            &op,
            || {
                checks += 1;
                if checks > 1 {
                    anyhow::bail!("Revoked");
                }
                Ok(())
            },
        )
        .unwrap_err();
    assert_eq!(error.to_string(), "Revoked");
    assert_eq!(checks, 2);
    assert_eq!(
        crate::object_store::read_object(&root.join("objects"), "TASK-1")
            .unwrap()
            .comments
            .len(),
        0
    );
}

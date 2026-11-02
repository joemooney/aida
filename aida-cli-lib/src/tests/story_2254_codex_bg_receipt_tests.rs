//! STORY-2254: private runtime receipts and detached scope admission.
// trace:STORY-2254 | ai:claude

use super::*;
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    _home: tempfile::TempDir,
    project: tempfile::TempDir,
    _env: crate::test_env::EnvVarsGuard,
}

fn fixture() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", home.path().to_str())]);
    Fixture {
        _home: home,
        project,
        _env: env,
    }
}

fn me() -> ProcId {
    ProcId::current().unwrap()
}

fn receipt_for(f: &Fixture, role: &str, spec: &str) -> LaunchReceipt {
    let attempt = uuid::Uuid::new_v4().to_string();
    LaunchReceipt::new(
        f.project.path(),
        &attempt,
        ScopeKey::new(role, spec),
        me(),
        "codex-implementer-01",
        f.project.path(),
        &f.project.path().join("log"),
    )
}

// trace:STORY-2254 | ai:claude
#[test]
fn story_2254_runtime_dir_and_receipts_are_private() {
    let f = fixture();
    let dir = runtime_dir(f.project.path()).unwrap();
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let r = receipt_for(&f, "implementer", "TASK-1");
    admit(&dir, &r).unwrap();
    let path = dir.join("attempts").join(format!("{}.json", r.attempt));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(dir.join("admission.lock").exists());
    // A group/world-accessible directory is refused, never used.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o640)).unwrap();
    assert!(runtime_dir(f.project.path()).is_err());
    assert!(existing_runtime_dir(f.project.path()).is_none());
    // Restore so the temp home can be removed.
    assert!(std::process::Command::new("chmod")
        .arg("700")
        .arg(&dir)
        .status()
        .unwrap()
        .success());
}

// trace:STORY-2254 | ai:claude
#[test]
fn story_2254_bg_two_same_scope_one_admitted_and_terminal_frees() {
    let f = fixture();
    let dir = runtime_dir(f.project.path()).unwrap();
    let first = receipt_for(&f, "implementer", "task-1");
    admit(&dir, &first).unwrap();
    let err = admit(&dir, &receipt_for(&f, "implementer", "TASK-1")).unwrap_err();
    assert!(format!("{err:#}").contains(&first.attempt), "{err:#}");
    // Another role on the same spec is another scope (review coexistence).
    admit(&dir, &receipt_for(&f, "reviewer", "TASK-1")).unwrap();
    // Every scope-holding phase blocks; the two terminal phases free it.
    for phase in [
        Phase::SupervisorOwned,
        Phase::Registered,
        Phase::AcceptedRunning,
        Phase::RecoveryRequired,
    ] {
        compare_update(
            &dir,
            f.project.path(),
            &first.attempt,
            |_| Ok(()),
            |r| r.phase = phase,
        )
        .unwrap();
        assert!(detached_scope_blocker(f.project.path(), "implementer", "TASK-1").is_some());
    }
    compare_update(
        &dir,
        f.project.path(),
        &first.attempt,
        |_| Ok(()),
        |r| r.phase = Phase::Exited,
    )
    .unwrap();
    assert!(detached_scope_blocker(f.project.path(), "implementer", "TASK-1").is_none());
    admit(&dir, &receipt_for(&f, "implementer", "TASK-1")).unwrap();
}

// trace:STORY-2254 | ai:claude
#[test]
fn story_2254_bg_corrupt_receipt_never_frees_scope() {
    let f = fixture();
    let dir = runtime_dir(f.project.path()).unwrap();
    let attempt = uuid::Uuid::new_v4().to_string();
    let path = dir.join("attempts").join(format!("{attempt}.json"));
    std::fs::write(&path, "{ not json").unwrap();
    assert!(matches!(
        read_receipt(&dir, f.project.path(), &attempt),
        ReceiptRead::Unknown(_)
    ));
    let blocker = detached_scope_blocker(f.project.path(), "implementer", "TASK-9").unwrap();
    assert!(blocker.contains("unreadable"), "{blocker}");
    assert!(admit(&dir, &receipt_for(&f, "implementer", "TASK-9")).is_err());
    // A symlinked receipt is not ours either.
    std::fs::remove_file(&path).unwrap();
    let target = f.project.path().join("elsewhere.json");
    std::fs::write(&target, "{}").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(matches!(
        read_receipt(&dir, f.project.path(), &attempt),
        ReceiptRead::Unknown(_)
    ));
    // Unknown is never updated.
    assert!(compare_update(
        &dir,
        f.project.path(),
        &attempt,
        |_| Ok(()),
        |r| r.phase = Phase::Exited
    )
    .is_err());
}

// trace:STORY-2254 | ai:claude
#[test]
fn story_2254_bg_owner_transfer_is_a_compare_update() {
    let f = fixture();
    let dir = runtime_dir(f.project.path()).unwrap();
    let r = receipt_for(&f, "implementer", "TASK-2");
    admit(&dir, &r).unwrap();
    assert!(dispatcher_still_owns(
        &dir,
        f.project.path(),
        &r.attempt,
        &me()
    ));
    let sup = ProcId {
        pid: 4242,
        start: "linux-starttime:42".into(),
    };
    let moved = compare_update(
        &dir,
        f.project.path(),
        &r.attempt,
        |now| {
            anyhow::ensure!(now.owner == Owner::Dispatcher(me()) && now.phase == Phase::Prepared);
            Ok(())
        },
        |now| {
            now.owner = Owner::Supervisor(sup.clone());
            now.phase = Phase::SupervisorOwned;
        },
    )
    .unwrap();
    assert_eq!(moved.generation, r.generation + 1);
    // After transfer the dispatcher's rollback guard owns nothing.
    assert!(!dispatcher_still_owns(
        &dir,
        f.project.path(),
        &r.attempt,
        &me()
    ));
    // A stale writer expecting the old owner changes nothing.
    let err = compare_update(
        &dir,
        f.project.path(),
        &r.attempt,
        |now| {
            anyhow::ensure!(now.owner == Owner::Dispatcher(me()), "not owned");
            Ok(())
        },
        |now| now.phase = Phase::RefusedBeforeExecution,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("not owned"));
    match read_receipt(&dir, f.project.path(), &r.attempt) {
        ReceiptRead::Known(now) => assert_eq!(now.phase, Phase::SupervisorOwned),
        ReceiptRead::Unknown(why) => panic!("{why}"),
    }
}

// trace:STORY-2254 | ai:claude
#[test]
fn story_2254_receipt_holds_no_secrets_or_env() {
    let f = fixture();
    let dir = runtime_dir(f.project.path()).unwrap();
    let r = receipt_for(&f, "implementer", "TASK-3");
    admit(&dir, &r).unwrap();
    let body =
        std::fs::read_to_string(dir.join("attempts").join(format!("{}.json", r.attempt))).unwrap();
    for forbidden in ["AIDA_", "token", "nonce", "prompt", "env"] {
        assert!(!body.contains(forbidden), "{forbidden} in {body}");
    }
}

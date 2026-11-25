//! STORY-2254: private runtime receipts and detached scope admission.
// trace:STORY-2254 | ai:claude
// trace:BUG-2476 | ai:codex

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
    admit(&dir, &r, || Ok(())).unwrap();
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
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_two_same_scope_one_admitted_and_terminal_frees() {
    let f = fixture();
    let dir = runtime_dir(f.project.path()).unwrap();
    let first = receipt_for(&f, "implementer", "task-1");
    admit(&dir, &first, || Ok(())).unwrap();
    let err = admit(&dir, &receipt_for(&f, "implementer", "TASK-1"), || Ok(())).unwrap_err();
    assert!(format!("{err:#}").contains(&first.attempt), "{err:#}");
    // Another role on the same spec is another scope (review coexistence).
    admit(&dir, &receipt_for(&f, "reviewer", "TASK-1"), || Ok(())).unwrap();
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
            |r| {
                r.phase = phase;
                r.worker = Some(me());
                r.supervisor = Some(me());
            },
        )
        .unwrap();
        assert!(scope_entry_blocker(f.project.path(), "implementer", "TASK-1").is_some());
    }
    compare_update(
        &dir,
        f.project.path(),
        &first.attempt,
        |_| Ok(()),
        |r| r.phase = Phase::Exited,
    )
    .unwrap();
    assert!(scope_entry_blocker(f.project.path(), "implementer", "TASK-1").is_none());
    admit(&dir, &receipt_for(&f, "implementer", "TASK-1"), || Ok(())).unwrap();
}

// trace:BUG-2748 | ai:codex
#[test]
fn story_2254_bg_legacy_prepared_recovery_stays_released() {
    let f = fixture();
    let dir = runtime_dir(f.project.path()).unwrap();
    let mut r = receipt_for(&f, "implementer", "TASK-1");
    r.phase = Phase::Exited;
    r.owner = Owner::Released;
    r.generation = 1;
    r.reason =
        Some("reconciled by `aida agent recover-launch`; worktree, branch and lease kept".into());
    r.terminal = Some(TerminalRecord {
        unknown: Some("recovered after its processes were gone".into()),
        ..Default::default()
    });
    write_receipt(&dir, &r).unwrap();
    assert!(scope_blocker(&dir, f.project.path(), &r.scope).is_none());
    r.reason = None;
    write_receipt(&dir, &r).unwrap();
    assert!(scope_blocker(&dir, f.project.path(), &r.scope).is_some());
}

/// A corrupt receipt never frees the scope it names, and one whose scope
/// cannot be read blocks every scope only while something may still run for
/// its attempt; a provably dead one is retired explicitly, kept as evidence.
// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn story_2254_bg_corrupt_receipt_never_frees_scope() {
    let f = fixture();
    let dir = runtime_dir(f.project.path()).unwrap();
    let attempt = uuid::Uuid::new_v4().to_string();
    let path = dir.join("attempts").join(format!("{attempt}.json"));
    // Readable scope, wrong version: blocks exactly that scope.
    std::fs::write(
        &path,
        r#"{"version": 99, "scope": {"role": "implementer", "spec": "TASK-9"}}"#,
    )
    .unwrap();
    assert!(matches!(
        read_receipt(&dir, f.project.path(), &attempt),
        ReceiptRead::Unknown(_)
    ));
    let blocker = scope_entry_blocker(f.project.path(), "implementer", "TASK-9").unwrap();
    assert!(blocker.contains("unreadable"), "{blocker}");
    assert!(admit(&dir, &receipt_for(&f, "implementer", "TASK-9"), || Ok(())).is_err());
    assert!(
        scope_entry_blocker(f.project.path(), "implementer", "TASK-10").is_none(),
        "one corrupt receipt must not block every scope"
    );
    std::fs::write(&path, "{ not json").unwrap();
    assert!(scope_entry_blocker(f.project.path(), "implementer", "TASK-10").is_some());
    let err = retire_unknown(&dir, f.project.path(), &attempt, Path::new("/proc")).unwrap_err();
    assert!(
        format!("{err:#}").contains("cannot be proven dead"),
        "{err:#}"
    );
    // trace:BUG-2748 | ai:codex
    let complete = serde_json::json!({
        "grant_id": "public-selector",
        "owner": Owner::Dispatcher(me()),
        "worker": {"pid": 2147483647, "start": me().start},
        "supervisor": {"pid": 2147483646, "start": me().start},
        "worktree": f.project.path(),
    });
    for missing in ["worker", "supervisor", "worktree", "owner", "all"] {
        let mut partial = complete.clone();
        partial
            .as_object_mut()
            .unwrap()
            .retain(|key, _| key == "grant_id" || (missing != "all" && key != missing));
        std::fs::write(&path, partial.to_string()).unwrap();
        assert!(matches!(
            attempt_occupancy(&dir, &attempt, Path::new("/proc")),
            Occupancy::Unknown(ref reason) if reason.contains("incomplete receipt attribution")
        ));
        let err = admit(&dir, &receipt_for(&f, "implementer", "TASK-10"), || Ok(())).unwrap_err();
        let inspect = format!("aida agent launch-status {attempt}");
        assert!(err.to_string().contains(&inspect), "{missing}: {err}");
        let err = retire_unknown(&dir, f.project.path(), &attempt, Path::new("/proc")).unwrap_err();
        assert!(err.to_string().contains(&inspect), "{missing}: {err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), partial.to_string());
    }
    // Provably dead: no longer blocks, and retires out of admission.
    std::fs::write(&path, complete.to_string()).unwrap();
    let proc_root = f.project.path(); // Mock process table containing only the inspector.
    std::fs::create_dir(proc_root.join(std::process::id().to_string())).unwrap();
    let scope = ScopeKey::new("implementer", "TASK-10");
    let blocker = || scope_blocker_except(&dir, proc_root, &scope, None, proc_root);
    let process = proc_root.join("2147483647");
    std::fs::create_dir(&process).unwrap();
    std::fs::write(process.join("stat"), "42 (worker) S 1 2147483647 1").unwrap();
    assert!(blocker().is_some());
    std::fs::write(process.join("stat"), "malformed").unwrap();
    assert!(blocker().is_some());
    std::fs::remove_dir_all(process).unwrap();
    assert!(blocker().is_none());
    let mut transferred = complete.clone();
    transferred["owner"] = transferred["supervisor"].clone();
    transferred["owner"]["kind"] = "supervisor".into();
    std::fs::write(&path, transferred.to_string()).unwrap();
    let earlier = proc_root.join("123456");
    std::fs::create_dir(&earlier).unwrap();
    std::fs::write(
        earlier.join("stat"),
        format!("123456 (helper) S 1 1 1 {} 0", "0 ".repeat(15)),
    )
    .unwrap();
    std::fs::create_dir(earlier.join("cmdline")).unwrap(); // EISDIR, even as root
    assert!(
        matches!(attempt_occupancy(&dir, &attempt, proc_root), Occupancy::Unknown(why) if why.contains("command line"))
    );
    assert!(blocker().is_some());
    assert!(retire_unknown(&dir, proc_root, &attempt, proc_root).is_err());
    std::fs::remove_dir_all(earlier).unwrap();
    let kept = retire_unknown(&dir, f.project.path(), &attempt, proc_root).unwrap();
    assert!(kept.starts_with(dir.join("retired")) && kept.exists());
    assert!(!path.exists());
    // A readable receipt is recovered, never retired.
    let r = receipt_for(&f, "implementer", "TASK-11");
    admit(&dir, &r, || Ok(())).unwrap();
    assert!(retire_unknown(&dir, f.project.path(), &r.attempt, Path::new("/proc")).is_err());
    // A symlinked receipt is not ours either.
    let attempt = uuid::Uuid::new_v4().to_string();
    let path = dir.join("attempts").join(format!("{attempt}.json"));
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

/// Terminal receipts are bounded by age and count; a receipt that still
/// holds its scope, or an unknown one, is never pruned.
// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_terminal_receipt_retention_is_bounded() {
    let f = fixture();
    let dir = runtime_dir(f.project.path()).unwrap();
    let held = receipt_for(&f, "implementer", "TASK-1");
    admit(&dir, &held, || Ok(())).unwrap();
    let mut terminal = Vec::new();
    for i in 0..(TERMINAL_KEEP_MAX + 3) {
        let r = receipt_for(&f, "implementer", &format!("TASK-{}", 100 + i));
        admit(&dir, &r, || Ok(())).unwrap();
        compare_update(
            &dir,
            f.project.path(),
            &r.attempt,
            |_| Ok(()),
            |r| {
                r.phase = Phase::RefusedBeforeExecution;
                r.owner = Owner::Released;
            },
        )
        .unwrap();
        terminal.push(r.attempt);
    }
    // The admissions above already pruned down to the count bound.
    let now = chrono::Utc::now();
    prune_terminal_receipts(&dir, f.project.path(), now);
    let remaining = |dir: &std::path::Path| list_receipts(dir, f.project.path()).len();
    assert_eq!(
        remaining(&dir),
        TERMINAL_KEEP_MAX + 1,
        "count bound plus the held one"
    );
    // Age bound: everything terminal goes, the held attempt stays.
    let later = now + TERMINAL_RETENTION + chrono::Duration::days(1);
    prune_terminal_receipts(&dir, f.project.path(), later);
    let left = list_receipts(&dir, f.project.path());
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].0, held.attempt);
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_owner_transfer_is_a_compare_update() {
    let f = fixture();
    let dir = runtime_dir(f.project.path()).unwrap();
    let r = receipt_for(&f, "implementer", "TASK-2");
    admit(&dir, &r, || Ok(())).unwrap();
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
            now.worker = Some(me());
            now.supervisor = Some(sup.clone());
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
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac3769a0 | ai:claude
#[test]
fn story_2254_receipt_holds_no_secrets_or_env() {
    let f = fixture();
    let dir = runtime_dir(f.project.path()).unwrap();
    let r = receipt_for(&f, "implementer", "TASK-3");
    admit(&dir, &r, || Ok(())).unwrap();
    let body =
        std::fs::read_to_string(dir.join("attempts").join(format!("{}.json", r.attempt))).unwrap();
    for forbidden in ["AIDA_", "token", "nonce", "prompt", "env"] {
        assert!(!body.contains(forbidden), "{forbidden} in {body}");
    }
}

fn write_scope_lease(root: &Path, id: &str, scope: &str) {
    let sessions = root.join(".aida/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let mut lease = toml::Table::new();
    for (key, value) in [
        ("id", id.to_string()),
        ("scope", scope.to_string()),
        ("slug", "s".into()),
        ("owner", "test".into()),
        ("worktree_path", root.display().to_string()),
        ("branch", "b".into()),
        ("started_at", "2026-11-02T00:00:00Z".into()),
        ("hostname", "test".into()),
        ("role", "implementer".into()),
    ] {
        lease.insert(key.into(), toml::Value::String(value));
    }
    if let Some(namespace) = aida_core::pid_observer::current_pid_namespace() {
        lease.insert(
            "pid_namespace".into(),
            toml::Value::Integer(namespace as i64),
        );
    }
    std::fs::write(
        sessions.join(format!("{id}.toml")),
        toml::to_string(&lease).unwrap(),
    )
    .unwrap();
}

/// Cross-route admission: another route's lease write for a scope a
/// detached attempt holds is refused INSIDE the admission lock (its write
/// never runs); the dispatcher's own lease write is exempt; and a detached
/// admission checks live leases inside the same lock, so whichever of the two
/// takes the lock first wins and the other refuses.
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_cross_route_admission_is_checked_inside_the_lock() {
    let f = fixture();
    let root = f.project.path();
    let dir = runtime_dir(root).unwrap();
    let held = receipt_for(&f, "implementer", "TASK-3");
    admit(&dir, &held, || Ok(())).unwrap();
    let mut wrote = false;
    let err = with_scope_entry(root, "implementer", "task-3", || {
        wrote = true;
        Ok(())
    })
    .unwrap_err();
    assert!(!wrote, "a refused route never writes its lease");
    assert!(format!("{err:#}").contains(&held.attempt), "{err:#}");
    // Another role's scope is untouched (review coexistence).
    with_scope_entry(root, "reviewer", "TASK-3", || Ok(())).unwrap();
    // The dispatcher's own lease write for its admitted attempt is exempt.
    {
        let _own = OwnAttemptScope::enter(&held.attempt);
        with_scope_entry(root, "implementer", "TASK-3", || Ok(())).unwrap();
    }
    assert!(with_scope_entry(root, "implementer", "TASK-3", || Ok(())).is_err());
    // The other order: a route's lease lands first, then a detached admission
    // refuses on it inside the lock.
    with_scope_entry(root, "implementer", "TASK-4", || {
        write_scope_lease(root, "lease-task-4", "TASK-4");
        Ok(())
    })
    .unwrap();
    let late = receipt_for(&f, "implementer", "TASK-4");
    let err = admit(&dir, &late, || {
        crate::agent_launch::agent_codex_background::refuse_live_scope_lease(
            root,
            &ScopeKey::new("implementer", "TASK-4"),
        )
    })
    .unwrap_err();
    assert!(format!("{err:#}").contains("TASK-4"), "{err:#}");
    assert!(
        matches!(
            read_receipt(&dir, root, &late.attempt),
            ReceiptRead::Unknown(_)
        ),
        "a refused admission writes no receipt"
    );
    // A read-only route (the role-enter resume picker) sees the hold too.
    assert!(scope_entry_blocker(root, "implementer", "TASK-3").is_some());
}

/// Every route that writes a lease, or offers to resume into a scope,
/// consults the shared admission record (source ratchet).
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_every_lease_route_consults_admission() {
    let session_start = include_str!("../session_start.rs");
    let at_write = session_start
        .split("crate::seat_clone::failpoint::check_before_lease()?;")
        .nth(1)
        .unwrap();
    let write = at_write
        .find("aida_core::write_atomic(&lease_file")
        .unwrap();
    assert!(
        at_write[..write].contains("codex_bg_receipt::with_scope_entry("),
        "session start's lease write runs inside the admission check"
    );
    assert!(include_str!("../claim.rs").contains("codex_bg_receipt::with_scope_entry("));
    assert!(include_str!("../role_cmd.rs").contains("codex_bg_receipt::scope_entry_blocker("));
    let launch = include_str!("../agent_launch.rs");
    assert!(launch.contains("codex_bg_receipt::scope_entry_blocker("));
    assert!(!launch.contains("detached_scope_blocker("));
}

use super::*;
use crate::process_probe::{fixture::FakeProc, ProbeSource};

// trace:STORY-2537 trace:BUG-2458 | ai:codex
#[test]
fn proc_identity_namespace_reuse_and_clock_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let proc = FakeProc::new(dir.path(), "11111111-1111-4111-8111-111111111111");
    proc.add(42, 1, "worker (with) spaces", 'S', 500, &[]);
    let source = ProbeSource::at(dir.path());
    let old = source.capture(42).unwrap();
    for namespace in [crate::process_probe::INITIAL_PID_NAMESPACE_INODE, 1234] {
        let id = ProcIdentity::from_record(&old, Some(namespace)).unwrap();
        let facts = ObserverFacts {
            same_node: true,
            boot_id: Some("11111111-1111-4111-8111-111111111111"),
            pidns_inode: Some(namespace),
            complete_procfs: true,
        };
        assert_eq!(judge(&id, &facts, || source.probe(&old)), Verdict::Alive);
        // Wall-time/btime changes never enter identity comparison.
        for btime in ["btime 1\n", "btime 9999999999\n"] {
            std::fs::write(dir.path().join("stat"), btime).unwrap();
            assert_eq!(judge(&id, &facts, || source.probe(&old)), Verdict::Alive);
        }
        for foreign in [namespace + 1, namespace + 2] {
            let other = ObserverFacts {
                pidns_inode: Some(foreign),
                ..facts
            };
            assert_eq!(
                judge(&id, &other, || panic!("foreign numeric PID lookup")),
                Verdict::ForeignUnknown(UnknownReason::NamespaceMismatch)
            );
        }
        let masked = ObserverFacts {
            complete_procfs: false,
            ..facts
        };
        assert_eq!(
            judge(&id, &masked, || panic!("masked view lookup")),
            Verdict::ForeignUnknown(UnknownReason::IncompleteProcfs)
        );
        let foreign_node = ObserverFacts {
            same_node: false,
            boot_id: Some("other-boot"),
            ..facts
        };
        assert_eq!(
            judge(&id, &foreign_node, || panic!("other node lookup")),
            Verdict::ForeignUnknown(UnknownReason::OtherNode)
        );
        let reboot = ObserverFacts {
            boot_id: Some("22222222-2222-4222-8222-222222222222"),
            ..facts
        };
        assert_eq!(
            judge(&id, &reboot, || panic!("reboot lookup")),
            Verdict::Dead(DeadReason::Reboot)
        );
        let missing = ObserverFacts {
            pidns_inode: None,
            ..facts
        };
        assert_eq!(
            judge(&id, &missing, || panic!("missing coordinates lookup")),
            Verdict::ForeignUnknown(UnknownReason::MissingIdentity)
        );
    }
    let mut corrupt = ProcIdentity::from_record(&old, Some(1234)).unwrap();
    corrupt.boot_id = "malformed-boot".into();
    let facts = ObserverFacts {
        same_node: true,
        boot_id: Some("11111111-1111-4111-8111-111111111111"),
        pidns_inode: Some(1234),
        complete_procfs: true,
    };
    assert_eq!(
        judge(&corrupt, &facts, || panic!("corrupt tuple lookup")),
        Verdict::ForeignUnknown(UnknownReason::MissingIdentity)
    );
    let id = ProcIdentity::from_record(&old, Some(1234)).unwrap();
    let facts = ObserverFacts {
        same_node: true,
        boot_id: Some("11111111-1111-4111-8111-111111111111"),
        pidns_inode: Some(1234),
        complete_procfs: true,
    };
    proc.add(42, 2013, "worker (with) spaces", 'S', 500, &[]);
    assert_eq!(
        judge(&id, &facts, || source.probe(&old)),
        Verdict::Alive,
        "reparenting must not replace worker identity"
    );
    assert!(invalid_holder_target(2013, &[2013], "systemd", "systemd"));
    assert!(invalid_holder_target(4321, &[4321, 1], "worker", "worker"));
    assert!(!invalid_holder_target(42, &[42], "worker", "worker"));
    proc.add(42, 1, "replacement", 'S', 501, &[]);
    assert!(matches!(
        judge(&id, &facts, || source.probe(&old)),
        Verdict::Dead(_)
    ));
    std::fs::write(dir.path().join("42/stat"), "malformed").unwrap();
    assert!(matches!(
        judge(&id, &facts, || source.probe(&old)),
        Verdict::ForeignUnknown(UnknownReason::Unreadable(_))
    ));
    assert!(ProcIdentity::from_record(
        &ProcessIdentity {
            pid: 42,
            start: "2026-11-05T00:00:00Z".into()
        },
        Some(1234)
    )
    .is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn proc_identity_cooperative_worker_and_subreaper() {
    use std::os::unix::process::CommandExt;
    let observable = LocalView::read().is_ok() && local_node_id().is_some();
    let mut cmd = std::process::Command::new("/bin/sleep");
    cmd.arg("30");
    let capture = prepare_command(&mut cmd).unwrap();
    let mut child = cmd.spawn().unwrap();
    let record = capture.finish(&mut child);
    // A complete local observer must exercise positive cooperative admission.
    if observable {
        assert!(record.validate().is_ok(), "{record:?}");
    }
    // A constrained observer can refuse, but it must explain why.
    if let Ok(id) = record.validate() {
        assert_eq!(id.pid, child.id());
        assert_eq!(record.observe().unwrap(), Verdict::Alive);
        let encoded = serde_json::to_string(&record).unwrap();
        assert!(recorded_holder(
            &encoded,
            Some(id.pid + 1),
            Some(&format!("linux-starttime:{}", id.starttime)),
            Some(id.pidns_inode)
        )
        .is_err());
    } else {
        assert!(matches!(
            record.refusal(),
            Some(HolderRefusal::EvidenceUnavailable(_))
        ));
    }
    // The core lease reader must select the admitted child rather than its
    // still-live creator, including after the child exits.
    // trace:STORY-2537 trace:BUG-2458 | ai:codex
    let lease = record.validate().ok().map(|id| {
        let lease: crate::liveness::SessionLeaseLite = serde_json::from_value(serde_json::json!({
            "scope": "BUG-2458",
            "started_at": chrono::Utc::now(),
            "holder_record": serde_json::to_string(&record).unwrap(),
            "active_pid": child.id(),
            "active_pid_start_time": format!("linux-starttime:{}", id.starttime),
            "pid_namespace": id.pidns_inode,
            "creator_pid": std::process::id(),
        }))
        .unwrap();
        assert_eq!(
            crate::liveness::lease_state_for(&lease, &[], chrono::Utc::now()),
            crate::liveness::LeaseState::Live
        );
        lease
    });
    child.kill().unwrap();
    child.wait().unwrap();
    if record.validate().is_ok() {
        assert!(matches!(record.observe().unwrap(), Verdict::Dead(_)));
        assert_eq!(
            crate::liveness::lease_state_for(&lease.unwrap(), &[], chrono::Utc::now()),
            crate::liveness::LeaseState::Stale
        );
    }

    let mut cmd = std::process::Command::new("/bin/sleep");
    cmd.arg("30");
    // SAFETY: child-only, async-signal-safe prctl.
    unsafe {
        cmd.pre_exec(|| {
            if libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let capture = prepare_command(&mut cmd).unwrap();
    let mut child = cmd.spawn().unwrap();
    let record = capture.finish(&mut child);
    assert!(
        record.validate().is_err(),
        "custom-named subreaper admitted"
    );
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(
        HolderRecord::capture(1).refusal(),
        Some(&HolderRefusal::InitOrManager)
    );
    assert!(HolderRecord::capture(std::process::id() + 1)
        .validate()
        .is_err());
}

// trace:TASK-3365 | ai:codex
#[cfg(target_os = "linux")]
#[test]
fn proc_identity_retained_child_can_be_recorded_after_exit() {
    let mut child = std::process::Command::new("/bin/true").spawn().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if matches!(
            read_stat(Path::new("/proc"), child.id()),
            Ok(StatRead::Found { state: 'Z', .. })
        ) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "child did not exit");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        ProcIdentity::capture(child.id()).is_err(),
        "ordinary capture still requires liveness"
    );
    let id = ProcIdentity::capture_child(&child).unwrap();
    assert_eq!(id.pid, child.id());
    assert_eq!(observe(&id, true), Verdict::Dead(DeadReason::Exited));
    assert!(child.wait().unwrap().success());
    assert!(
        ProcIdentity::capture_child(&child).is_err(),
        "reaped child is not recordable"
    );
}

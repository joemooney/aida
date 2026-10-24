//! BUG-2130 regression tests: exact `/proc/<pid>/stat` tick start-time identity
//! across lock writers/readers and mixed legacy-RFC3339 / exact-tick equality
//! sites (`worktree_gc`, `leases`, `coordination`, `seat_occupancy::runtime`,
//! `review_authority`, `drain_lock`, `solo_lock`, and `relay_stall`).
// trace:BUG-2130 | ai:agy

use std::collections::HashSet;

use crate::leases::SessionLease;

fn sample_lease(creator_pid: Option<u32>, creator_start: Option<&str>) -> SessionLease {
    let mut raw = String::from(
        "id = \"019200000000\"\n\
         scope = \"BUG-2130\"\n\
         slug = \"bug-2130\"\n\
         owner = \"test@example.com\"\n\
         worktree_path = \"nonexistent-bug-2130\"\n\
         branch = \"claude/bug-2130\"\n\
         started_at = \"2026-10-18T00:00:00Z\"\n\
         hostname = \"testhost\"\n",
    );
    if let Some(pid) = creator_pid {
        raw.push_str(&format!("creator_pid = {pid}\n"));
    }
    if let Some(start) = creator_start {
        raw.push_str(&format!("creator_pid_start_time = {start:?}\n"));
    }
    toml::from_str(&raw).expect("valid SessionLease TOML")
}

// trace:BUG-2130 | ai:agy
#[test]
fn bug_2130_worktree_gc_owner_is_caller_matches_legacy_lease_and_exact_caller() {
    let me = std::process::id();
    let legacy = aida_core::liveness::process_start_identity_legacy(me).expect("own legacy start");
    let exact = aida_core::liveness::process_start_identity(me).expect("own exact start");
    assert!(
        exact.starts_with(aida_core::liveness::NATIVE_EXACT_SCHEME),
        "{exact}"
    );
    assert_ne!(legacy, exact);

    let lease = sample_lease(Some(me), Some(&legacy));
    let facts = crate::worktree_gc::probe_lease_death_facts(
        &lease,
        "testhost",
        Some((me, Some(exact.as_str()))),
        &HashSet::new(),
    );
    assert!(
        facts.owner_is_caller,
        "pre-upgrade lease with legacy RFC3339 creator_pid_start_time must still recognize live caller with exact start identity"
    );

    let wrong_lease = sample_lease(Some(me), Some("1999-01-01T00:00:00+00:00"));
    let wrong_facts = crate::worktree_gc::probe_lease_death_facts(
        &wrong_lease,
        "testhost",
        Some((me, Some(exact.as_str()))),
        &HashSet::new(),
    );
    assert!(
        !wrong_facts.owner_is_caller,
        "mismatched creator_pid_start_time must not match caller"
    );
}

// trace:BUG-2130 | ai:agy
#[test]
fn bug_2130_lease_minted_by_returned_phase_child_matches_mixed_formats() {
    let me = std::process::id();
    let legacy = aida_core::liveness::process_start_identity_legacy(me).expect("own legacy start");
    let exact = aida_core::liveness::process_start_identity(me).expect("own exact start");

    let lease = sample_lease(Some(me), Some(&legacy));
    assert!(
        crate::leases::lease_minted_by_returned_phase_child(&lease, me, Some(exact.as_str())),
        "pre-upgrade phase-child lease with legacy start time must match live orchestrator with exact start identity"
    );

    let recycled = sample_lease(Some(me), Some("1999-01-01T00:00:00+00:00"));
    assert!(
        !crate::leases::lease_minted_by_returned_phase_child(&recycled, me, Some(exact.as_str())),
        "recycled pid with mismatched start identity must not qualify"
    );
}

// trace:BUG-2130 | ai:agy
#[test]
fn bug_2130_adopt_spec_lease_for_subagent_reclaims_recycled_active_pid_and_preserves_live() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let sessions_dir = root.join(".aida").join("sessions");
    let worktree = root.join("wt-bug-2130");
    std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
    std::fs::create_dir_all(&worktree).expect("create worktree dir");

    let me = std::process::id();
    let legacy = aida_core::liveness::process_start_identity_legacy(me).expect("own legacy start");
    let lease_path = sessions_dir.join("lease-2130.toml");

    // 1. Recycled active_pid (mismatched active_pid_start_time) -> subagent adopts the lease.
    let recycled_toml = format!(
        "id = \"lease-2130\"\n\
         scope = \"BUG-2130\"\n\
         slug = \"bug-2130\"\n\
         owner = \"test@example.com\"\n\
         worktree_path = {:?}\n\
         branch = \"claude/bug-2130\"\n\
         started_at = \"2026-10-18T00:00:00Z\"\n\
         hostname = \"testhost\"\n\
         active_pid = {me}\n\
         active_pid_start_time = \"1999-01-01T00:00:00+00:00\"\n",
        worktree.to_string_lossy()
    );
    std::fs::write(&lease_path, &recycled_toml).expect("write recycled lease");

    let adopted = crate::leases::adopt_spec_lease_for_subagent(root, &worktree, Some(me));
    assert_eq!(
        adopted.as_deref(),
        Some("lease-2130"),
        "recycled active_pid must allow subagent adoption"
    );
    let updated_raw = std::fs::read_to_string(&lease_path).expect("read adopted lease");
    let updated_val: toml::Value = toml::from_str(&updated_raw).expect("parse adopted lease");
    let new_start = updated_val
        .get("active_pid_start_time")
        .and_then(|v| v.as_str())
        .expect("updated active_pid_start_time");
    assert!(
        new_start.starts_with(aida_core::liveness::NATIVE_EXACT_SCHEME),
        "adopted lease must stamp exact start identity, got {new_start}"
    );

    // 2. Live active_pid with matching exact identity -> adoption skipped (returns None).
    assert_eq!(
        crate::leases::adopt_spec_lease_for_subagent(root, &worktree, Some(me)),
        None,
        "live active_pid with matching exact identity must block adoption"
    );

    // 3. Live active_pid with pre-upgrade legacy RFC3339 identity -> adoption also skipped.
    let legacy_live_toml = format!(
        "id = \"lease-2130\"\n\
         scope = \"BUG-2130\"\n\
         slug = \"bug-2130\"\n\
         owner = \"test@example.com\"\n\
         worktree_path = {:?}\n\
         branch = \"claude/bug-2130\"\n\
         started_at = \"2026-10-18T00:00:00Z\"\n\
         hostname = \"testhost\"\n\
         active_pid = {me}\n\
         active_pid_start_time = {legacy:?}\n",
        worktree.to_string_lossy()
    );
    std::fs::write(&lease_path, &legacy_live_toml).expect("write legacy live lease");
    assert_eq!(
        crate::leases::adopt_spec_lease_for_subagent(root, &worktree, Some(me)),
        None,
        "live active_pid with matching legacy RFC3339 identity must block adoption"
    );
}

// trace:BUG-2130 | ai:agy
#[test]
fn bug_2130_coordination_strictly_alive_accepts_both_exact_and_legacy_for_live_pid() {
    let me = std::process::id();
    let legacy = aida_core::liveness::process_start_identity_legacy(me).expect("own legacy start");
    let exact = aida_core::liveness::process_start_identity(me).expect("own exact start");

    assert!(crate::coordination::strictly_alive(me, Some(&exact)));
    assert!(crate::coordination::strictly_alive(me, Some(&legacy)));
    assert!(!crate::coordination::strictly_alive(me, None));
    assert!(!crate::coordination::strictly_alive(
        me,
        Some("linux-starttime:0")
    ));
    assert!(!crate::coordination::strictly_alive(
        me,
        Some("1999-01-01T00:00:00+00:00")
    ));
}

// trace:BUG-2130 | ai:agy
#[test]
fn bug_2130_review_authority_tokens_match_across_legacy_and_exact() {
    // Hermetic unit checks via tokens_match_with:
    let legacy_tok = "proc:42@2026-10-18T00:00:00+00:00";
    let exact_tok = "shell:42@linux-starttime:987654";

    // 1. Same live pid where both tokens strictly match the live process -> true.
    assert!(crate::review_authority::tokens_match_with(
        legacy_tok,
        exact_tok,
        |pid, rec| {
            pid == 42 && matches!(rec, "2026-10-18T00:00:00+00:00" | "linux-starttime:987654")
        }
    ));

    // 2. TASK-2049: a dead pid still matches across schemes, since a refusal
    // check must not depend on the author process staying alive; within one
    // scheme different starts stay distinct (a recycled pid).
    // trace:TASK-1951 trace:TASK-2049 | ai:claude
    assert!(crate::review_authority::tokens_match_with(
        legacy_tok,
        exact_tok,
        |_, _| false,
    ));
    assert!(!crate::review_authority::tokens_match_with(
        "proc:42@linux-starttime:1",
        "shell:42@linux-starttime:2",
        |_, _| false,
    ));
    assert!(!crate::review_authority::tokens_match_with(
        "proc:42@2026-10-18T00:00:00+00:00",
        "proc:42@2026-10-19T00:00:00+00:00",
        |_, _| false,
    ));
    assert!(!crate::review_authority::tokens_match_with(
        "proc:42@garbage",
        exact_tok,
        |_, _| false,
    ));
    // TASK-2136: the two exact schemes are distinct too, so a dead pid's
    // linux-starttime / sysinfo-start pair also matches without a probe.
    // trace:TASK-1951 trace:TASK-2136 | ai:claude
    assert!(crate::review_authority::tokens_match_with(
        "proc:42@linux-starttime:987654",
        "shell:42@sysinfo-start:1760745600",
        |_, _| false,
    ));
    assert!(!crate::review_authority::tokens_match_with(
        "proc:42@sysinfo-start:1",
        "shell:42@sysinfo-start:2",
        |_, _| false,
    ));

    // 3. Different pids -> false even if strict_match would return true.
    assert!(!crate::review_authority::tokens_match_with(
        "proc:42@2026-10-18T00:00:00+00:00",
        "proc:43@linux-starttime:987654",
        |_, _| true,
    ));

    // 4. End-to-end against our own live process:
    let me = std::process::id();
    let legacy = aida_core::liveness::process_start_identity_legacy(me).expect("own legacy start");
    let exact = aida_core::liveness::process_start_identity(me).expect("own exact start");
    assert!(crate::review_authority::tokens_match(
        &format!("proc:{me}@{legacy}"),
        &format!("shell:{me}@{exact}"),
    ));
}

// trace:BUG-2130 | ai:agy
#[test]
fn bug_2130_solo_and_drain_locks_write_exact_identity_and_read_back_live() {
    let _env = crate::drain_lock::test_env_isolation();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join(".aida")).expect("create .aida");

    let solo_guard = crate::solo_lock::acquire_solo_lock(root).expect("acquire solo lock");
    match crate::solo_lock::probe_lock(root) {
        crate::solo_lock::LockStatus::Running(lock) => {
            assert_eq!(lock.pid, std::process::id());
            let start = lock.pid_start_time.as_deref().expect("solo pid_start_time");
            assert!(
                start.starts_with(aida_core::liveness::NATIVE_EXACT_SCHEME),
                "solo lock must record exact tick identity, got {start}"
            );
        }
        other => panic!("expected Running solo lock, got {other:?}"),
    }
    drop(solo_guard);

    let drain_guard =
        crate::drain_lock::acquire_drain_lock(root, "queue work --auto-complete").expect("drain");
    let live_drain = crate::drain_lock::read_pid_live_lock(root).expect("live drain lock");
    assert_eq!(live_drain.pid, std::process::id());
    let drain_start = live_drain
        .pid_start_time
        .as_deref()
        .expect("drain pid_start_time");
    assert!(
        drain_start.starts_with(aida_core::liveness::NATIVE_EXACT_SCHEME),
        "drain lock must record exact tick identity, got {drain_start}"
    );
    drop(drain_guard);

    // Pre-upgrade legacy-RFC3339 solo and drain locks held by a live PID must
    // still report live and block acquisition.
    let me = std::process::id();
    let legacy = aida_core::liveness::process_start_identity_legacy(me).expect("own legacy start");

    let legacy_solo = crate::solo_lock::SoloLock {
        pid: me,
        pid_start_time: Some(legacy.clone()),
        started_at_utc: "2026-10-18T00:00:00Z".to_string(),
        host: "testhost".to_string(),
    };
    std::fs::write(
        crate::solo_lock::solo_lock_path(root),
        serde_json::to_string_pretty(&legacy_solo).expect("serialize legacy solo lock"),
    )
    .expect("write legacy solo lock");
    assert!(
        matches!(
            crate::solo_lock::probe_lock(root),
            crate::solo_lock::LockStatus::Running(_)
        ),
        "legacy-RFC3339 solo lock held by live PID must report Running"
    );
    assert!(
        crate::solo_lock::acquire_solo_lock(root).is_err(),
        "legacy-RFC3339 solo lock held by live PID must block acquire_solo_lock"
    );
    std::fs::remove_file(crate::solo_lock::solo_lock_path(root)).expect("remove legacy solo lock");

    let legacy_drain = crate::drain_lock::DrainLock {
        pid: me,
        pid_start_time: Some(legacy),
        started_at_utc: "2026-10-18T00:00:00Z".to_string(),
        command: "burndown run".to_string(),
        host: "testhost".to_string(),
        wave_id: "wave-legacy".to_string(),
        binary_sha: String::new(),
        binary_mtime_secs: None,
        binary_path: String::new(),
        launched_stale: false,
        specs: Vec::new(),
        child_grant_id: None,
    };
    std::fs::write(
        crate::drain_lock::drain_lock_path(root),
        serde_json::to_string_pretty(&legacy_drain).expect("serialize legacy drain lock"),
    )
    .expect("write legacy drain lock");
    assert!(
        crate::drain_lock::read_pid_live_lock(root).is_some(),
        "legacy-RFC3339 drain lock held by live PID must read back as live"
    );
    assert!(
        crate::drain_lock::acquire_drain_lock(root, "queue work --auto-complete").is_err(),
        "legacy-RFC3339 drain lock held by live PID must block acquire_drain_lock"
    );
    std::fs::remove_file(crate::drain_lock::drain_lock_path(root))
        .expect("remove legacy drain lock");
}

// trace:BUG-2130 | ai:agy
#[test]
fn bug_2130_relay_stall_owner_alive_parses_wall_clock_for_live_owner() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path();
    std::fs::write(run_dir.join("relay.lock"), "").expect("create relay.lock");

    let me = std::process::id();
    let future_updated = (chrono::Utc::now() + chrono::Duration::seconds(60)).to_rfc3339();
    let status = crate::fleet_lanes::parse_status(&format!(
        "state=running\npid={me}\nupdated={future_updated}\n"
    ));
    assert_eq!(
        crate::fleet_lanes::owner_alive(run_dir, &status),
        Some(true),
        "live owner whose process_start_time <= updated must report Some(true)"
    );

    let past_updated = "1999-01-01T00:00:00Z";
    let recycled_status = crate::fleet_lanes::parse_status(&format!(
        "state=running\npid={me}\nupdated={past_updated}\n"
    ));
    assert_eq!(
        crate::fleet_lanes::owner_alive(run_dir, &recycled_status),
        Some(false),
        "process started after relay.status updated timestamp must report Some(false)"
    );
}

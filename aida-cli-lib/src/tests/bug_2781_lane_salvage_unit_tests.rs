//! BUG-2781: which failed exits salvage, where to, and how the end is named.
// trace:BUG-2781 | ai:claude

use super::*;
use crate::headless_background::OriginHead;

fn seat(head: &str, origin: OriginHead, uncommitted: usize, unpushed: Option<bool>) -> SeatState {
    SeatState {
        head: Some(head.into()),
        origin,
        uncommitted: Some(uncommitted),
        head_has_unpushed: unpushed,
    }
}

#[test]
fn bug_2781_only_a_failed_exit_with_committed_unpushed_work_salvages() {
    let committed = seat("b", OriginHead::Absent, 0, Some(true));
    assert!(wanted(&Outcome::NoPr, false, &committed, Some("a")));
    // Committed work beside uncommitted edits: the commits salvage, the edits stay.
    let dirty = seat("b", OriginHead::At("a".into()), 2, Some(true));
    assert!(wanted(&Outcome::NoPr, false, &dirty, Some("a")));
    // A clean exit publishes the normal way.
    assert!(!wanted(&Outcome::NoPr, true, &committed, Some("a")));
    // Uncommitted only: HEAD never moved.
    let edits = seat("a", OriginHead::At("a".into()), 3, Some(false));
    assert!(!wanted(&Outcome::NoPr, false, &edits, Some("a")));
    // Already pushed, unreadable reachability, a punt or hold: no.
    let pushed = seat("b", OriginHead::At("b".into()), 0, Some(false));
    assert!(!wanted(&Outcome::NoPr, false, &pushed, Some("a")));
    let unknown = seat("b", OriginHead::Absent, 0, None);
    assert!(!wanted(&Outcome::NoPr, false, &unknown, Some("a")));
    let unknown_origin = seat("b", OriginHead::Unknown, 0, Some(true));
    assert!(!wanted(&Outcome::NoPr, false, &unknown_origin, Some("a")));
    let punted = Outcome::Punted { reason: "x".into() };
    assert!(!wanted(&punted, false, &committed, Some("a")));
}

#[test]
fn bug_2781_salvage_targets_the_pr_branch_only_when_a_pr_is_open() {
    assert_eq!(target("feat", Some(7), "lease-1"), "feat");
    assert_eq!(target("feat", None, "lease-1"), "salvage/lease-1");
}

#[cfg(unix)]
fn raw(code: i32) -> std::process::ExitStatus {
    std::os::unix::process::ExitStatusExt::from_raw(code)
}

#[cfg(unix)]
#[test]
fn bug_2781_exit_cause_names_the_signal_and_every_checked_source() {
    // An unreadable kernel log says so; it never claims "not recorded".
    let blind = EndEvidence::default();
    let term = exit_cause(raw(15), &blind);
    assert_eq!(
        term,
        "ended by SIGTERM (15); source: OOM unknown (kernel log unreadable)"
    );
    assert!(!term.contains("not recorded"), "{term}");
    // A wrapper's exit 143 is 128 + SIGTERM.
    let wrapped = exit_cause(raw(143 << 8), &blind);
    assert!(wrapped.starts_with("ended by SIGTERM (15)"), "{wrapped}");
    // A readable log with no OOM kill says it checked.
    let clean = EndEvidence {
        kernel_log: Some("2026-11-27T01:48:00+0000 host kernel: usb 1-1: reset\n".into()),
        ..EndEvidence::default()
    };
    let kill = exit_cause(raw(9), &clean);
    assert!(
        kill.contains("SIGKILL (9)") && kill.contains("no OOM kill in the kernel log"),
        "{kill}"
    );
    // A plain exit code names no source unless one was recorded.
    assert_eq!(exit_cause(raw(1 << 8), &blind), "exited with code 1");
    let text = announcement("abc", "salvage/l", &term);
    assert!(text.starts_with(UNVERIFIED), "{text}");
    assert!(
        text.contains("SIGTERM") && text.contains("independent review"),
        "{text}"
    );
}

#[cfg(unix)]
#[test]
fn bug_2781_exit_cause_names_the_oom_killer_when_the_kernel_log_matches() {
    let log = "2026-11-27T02:09:01+0000 host kernel: oom-kill:constraint=CONSTRAINT_NONE,\
               task=claude,pid=4242,uid=1000\n\
               2026-11-27T02:09:01+0000 host kernel: Out of memory: Killed process 4242 \
               (claude) total-vm:9000kB, anon-rss:8000kB\n\
               2026-11-27T02:09:02+0000 host kernel: Memory cgroup out of memory: Killed \
               process 5151 (cargo) total-vm:1kB\n";
    let agent = EndEvidence {
        agent_pid: Some(4242),
        kernel_log: Some(log.into()),
        ..EndEvidence::default()
    };
    let cause = exit_cause(raw(9), &agent);
    assert!(
        cause.starts_with(
            "ended by SIGKILL (9); source: the kernel OOM killer killed the agent (pid 4242)"
        ),
        "{cause}"
    );
    // The OOM killer took another process during the lane, not the agent.
    let other = EndEvidence {
        agent_pid: Some(7),
        kernel_log: Some(log.into()),
        ..EndEvidence::default()
    };
    let cause = exit_cause(raw(15), &other);
    assert!(
        cause.contains(
            "killed claude (pid 4242), cargo (pid 5151) during the lane, not the agent's pid"
        ),
        "{cause}"
    );
}

#[cfg(unix)]
#[test]
fn bug_2781_exit_cause_names_a_job_timeout_and_an_operator_stop() {
    let evidence = EndEvidence {
        job_timeouts: vec!["j1".into()],
        operator_stop: Some("the lease was interrupted at T (drain-sigterm)".into()),
        kernel_log: None,
        agent_pid: None,
    };
    let cause = exit_cause(raw(15), &evidence);
    assert_eq!(
        cause,
        "ended by SIGTERM (15); source: operator stop: the lease was interrupted at T \
         (drain-sigterm); lane-job timeout: j1 hit the launcher's job deadline; OOM unknown \
         (kernel log unreadable)"
    );
    let code = exit_cause(raw(1 << 8), &evidence);
    assert!(
        code.starts_with("exited with code 1; source: operator stop:"),
        "{code}"
    );
}

#[test]
fn bug_2781_gather_reads_the_lane_records_and_the_injected_kernel_log() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let since = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
    let at = |secs: i64| {
        chrono::DateTime::from_timestamp(1_800_000_000 + secs, 0)
            .unwrap()
            .to_rfc3339()
    };
    let jobs = crate::lane_job::results_dir(root, "lease-1");
    std::fs::create_dir_all(&jobs).unwrap();
    std::fs::write(
        jobs.join("jobs.log"),
        format!(
            "{} j0 timed out rc=124 after 900s\n{} j1 started: make (timeout 900s)\n\
             {} j1 timed out rc=124 after 900s; target overlay-stale\n\
             {} j2 stopped at lane end rc=137 after 12s\n",
            at(-60),
            at(1),
            at(901),
            at(902)
        ),
    )
    .unwrap();
    let lease = crate::leases::lease_path(root, "lease-1");
    std::fs::create_dir_all(lease.parent().unwrap()).unwrap();
    std::fs::write(
        &lease,
        format!(
            "id = \"lease-1\"\ninterrupted_at = \"{}\"\ninterrupted_reason = \"drain-sigterm\"\n",
            at(950)
        ),
    )
    .unwrap();
    let asked = std::cell::Cell::new(None);
    let evidence = gather(root, root, "lease-1", since, Some(42), &|t| {
        asked.set(Some(t));
        Some(String::new())
    });
    assert_eq!(
        asked.get(),
        Some(since),
        "the kernel log is read from the lane start"
    );
    // j0 timed out before this lane started; j2 was stopped, not timed out.
    assert_eq!(evidence.job_timeouts, vec!["j1".to_string()]);
    assert_eq!(evidence.agent_pid, Some(42));
    assert_eq!(evidence.kernel_log.as_deref(), Some(""));
    let stop = evidence.operator_stop.expect("lease interruption");
    assert!(
        stop.contains("interrupted") && stop.contains("drain-sigterm"),
        "{stop}"
    );

    // An older interruption and no lease: a drain stop request is the record.
    std::fs::remove_file(&lease).unwrap();
    let stop_file = crate::drain_cmd::drain_stop_path(root);
    std::fs::create_dir_all(stop_file.parent().unwrap()).unwrap();
    std::fs::write(
        &stop_file,
        format!(
            "{{\"requested_at_utc\": \"{}\", \"mode\": \"after-current\"}}",
            at(5)
        ),
    )
    .unwrap();
    let evidence = gather(root, root, "lease-1", since, None, &|_| None);
    assert_eq!(
        evidence.operator_stop.as_deref(),
        Some(
            format!(
                "a drain stop was requested at {} (mode after-current)",
                at(5)
            )
            .as_str()
        )
    );
    assert_eq!(evidence.kernel_log, None, "unreadable stays unknown");
    // A stop request from before this lane is not this lane's end.
    std::fs::write(
        &stop_file,
        format!(
            "{{\"requested_at_utc\": \"{}\", \"mode\": \"now\"}}",
            at(-5)
        ),
    )
    .unwrap();
    assert_eq!(
        gather(root, root, "lease-1", since, None, &|_| None).operator_stop,
        None
    );
}

#[test]
fn bug_2781_gate_end_line_records_wall_time_and_overlay_state() {
    let line = crate::lane_job::end_line("j1", "stopped at lane end", 137, 1288, Some(true));
    assert_eq!(
        line,
        "j1 stopped at lane end rc=137 after 1288s; target overlay-stale"
    );
    let fresh = crate::lane_job::end_line("j1", "exited", 0, 90, Some(false));
    assert!(
        fresh.ends_with("after 90s; target overlay fresh"),
        "{fresh}"
    );
    assert_eq!(
        crate::lane_job::end_line("j1", "exited", 0, 5, None),
        "j1 exited rc=0 after 5s"
    );
}

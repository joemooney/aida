use super::*;

// trace:STORY-545 | ai:claude
#[test]
fn skill_prompt_defaults_to_status_only() {
    assert_eq!(
        burndown_skill_prompt("approved", None, None, None, None),
        "/aida-burndown --status approved"
    );
}

#[test]
fn skill_prompt_appends_each_provided_knob_in_order() {
    assert_eq!(
        burndown_skill_prompt(
            "draft",
            Some("papercut"),
            Some("scaffold"),
            Some(10),
            Some(4)
        ),
        "/aida-burndown --status draft --tag papercut --batch scaffold --max 10 --concurrency 4"
    );
}

#[test]
fn skill_prompt_omits_unset_caps() {
    assert_eq!(
        burndown_skill_prompt("approved", None, None, None, Some(3)),
        "/aida-burndown --status approved --concurrency 3"
    );
}

// trace:TASK-804 | ai:claude
#[test]
fn verbose_args_carry_the_stream_json_flags_and_passthrough_mode() {
    let args = burndown_verbose_claude_args("/aida-burndown --status approved", "acceptEdits");
    // The mandatory stream-json trio so the tee can render live events.
    assert!(args.contains(&"--output-format".to_string()));
    assert!(args.contains(&"stream-json".to_string()));
    assert!(args.contains(&"--verbose".to_string()));
    assert!(args.contains(&"--include-partial-messages".to_string()));
    // Permission mode is caller-controlled — `--verbose` must not force
    // bypassPermissions over an explicit operator choice.
    let mode_pos = args.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(args[mode_pos + 1], "acceptEdits");
    // The prompt stays the trailing positional.
    assert_eq!(args.last().unwrap(), "/aida-burndown --status approved");
}

// trace:TASK-804 | ai:claude
#[test]
fn verbose_args_default_mode_is_passed_through_verbatim() {
    // The launcher resolves the default to `bypassPermissions` before
    // calling this builder; the builder itself just threads whatever it's
    // handed, never injecting a posture of its own.
    let args = burndown_verbose_claude_args("/aida-burndown", "bypassPermissions");
    let mode_pos = args.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(args[mode_pos + 1], "bypassPermissions");
}

// trace:TASK-804 | ai:claude
#[test]
fn drain_log_path_is_under_aida_burndown() {
    let p = burndown_drain_log_path(std::path::Path::new("/repo"), "20260613T120000Z-abcd1234");
    assert_eq!(
        p,
        std::path::PathBuf::from("/repo/.aida/burndown/20260613T120000Z-abcd1234.jsonl")
    );
}

// ── burndown status (read-side, TASK-806) ──

fn sample_lock(pid: u32) -> crate::drain_lock::DrainLock {
    crate::drain_lock::DrainLock {
        pid,
        started_at_utc: "2026-06-13T21:53:00Z".to_string(),
        command: "burndown run --status approved".to_string(),
        host: "devbox".to_string(),
    }
}

// trace:TASK-806 | ai:claude
#[test]
fn status_human_no_drain_says_so_and_hints_run() {
    let out = render_burndown_status_human(&crate::drain_lock::LockStatus::None, &[], None);
    assert!(out.contains("no drain running"), "out: {out}");
    assert!(out.contains("aida burndown run"), "out: {out}");
}

// trace:TASK-806 | ai:claude
#[test]
fn status_human_running_shows_pid_command_and_inflight() {
    let lock = crate::drain_lock::LockStatus::Running(sample_lock(4242));
    let in_flight = vec![InFlightLease {
        scope: "TASK-806".into(),
        branch: "task-806".into(),
        role: "implementer".into(),
        worktree: "/home/joe/ai/aida-task-806".into(),
        last_activity: None,
    }];
    let log = std::path::PathBuf::from("/repo/.aida/burndown/20260613T120000Z-abcd1234.jsonl");
    let out = render_burndown_status_human(&lock, &in_flight, Some(&log));
    assert!(out.contains("drain running"), "out: {out}");
    assert!(out.contains("4242"), "out: {out}");
    assert!(out.contains("burndown run --status approved"), "out: {out}");
    assert!(out.contains("In-flight (1 leased)"), "out: {out}");
    assert!(out.contains("TASK-806"), "out: {out}");
    assert!(out.contains("Live log"), "out: {out}");
    assert!(out.contains(".jsonl"), "out: {out}");
}

// trace:TASK-806 | ai:claude
#[test]
fn status_human_stale_lock_flags_crash() {
    let lock = crate::drain_lock::LockStatus::Stale(sample_lock(999));
    let out = render_burndown_status_human(&lock, &[], None);
    assert!(out.contains("stale drain lock"), "out: {out}");
    assert!(out.contains("999"), "out: {out}");
    assert!(out.contains("reclaims"), "out: {out}");
}

// trace:TASK-834 | ai:claude
#[test]
fn activity_label_active_vs_idle_stuck_thresholds() {
    let now = chrono::Utc::now();

    // Just committed → "active Ns ago", no stuck flag.
    let recent = now - chrono::Duration::seconds(15);
    let label = format_activity_label(recent, now);
    assert!(label.starts_with("active"), "label: {label}");
    assert!(label.contains("15s"), "label: {label}");
    assert!(!label.contains("stuck"), "label: {label}");

    // Just under the threshold (9m) is still active.
    let almost = now - chrono::Duration::seconds(ACTIVITY_STUCK_THRESHOLD_SECS - 60);
    let label = format_activity_label(almost, now);
    assert!(label.starts_with("active"), "label: {label}");
    assert!(!label.contains("stuck"), "label: {label}");

    // Past the threshold (15m) -> "idle 15m possibly stuck" with a warning marker.
    let stale = now - chrono::Duration::minutes(15);
    let label = format_activity_label(stale, now);
    assert!(label.starts_with("idle"), "label: {label}");
    assert!(label.contains("15m"), "label: {label}");
    assert!(label.contains("possibly stuck"), "label: {label}");

    // Exactly at the threshold flips to stuck (>=).
    let at = now - chrono::Duration::seconds(ACTIVITY_STUCK_THRESHOLD_SECS);
    assert!(
        format_activity_label(at, now).contains("stuck"),
        "boundary should be inclusive"
    );
}

// trace:TASK-806 | ai:claude
#[test]
fn status_json_running_carries_drain_inflight_and_log() {
    let lock = crate::drain_lock::LockStatus::Running(sample_lock(4242));
    let in_flight = vec![InFlightLease {
        scope: "TASK-806".into(),
        branch: "task-806".into(),
        role: "implementer".into(),
        worktree: "/wt".into(),
        last_activity: None,
    }];
    let log = std::path::PathBuf::from("/repo/.aida/burndown/run.jsonl");
    let s = render_burndown_status_json(&lock, &in_flight, Some(&log));
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["drain"]["running"], serde_json::json!(true));
    assert_eq!(v["drain"]["pid"], serde_json::json!(4242));
    assert_eq!(v["drain"]["command"], "burndown run --status approved");
    assert_eq!(v["in_flight"][0]["spec"], "TASK-806");
    assert_eq!(v["log"], "/repo/.aida/burndown/run.jsonl");
}

// trace:TASK-806 | ai:claude
#[test]
fn status_json_none_is_not_running_and_null_log() {
    let s = render_burndown_status_json(&crate::drain_lock::LockStatus::None, &[], None);
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["drain"]["running"], serde_json::json!(false));
    assert_eq!(v["log"], serde_json::Value::Null);
    assert!(v["in_flight"].as_array().unwrap().is_empty());
}

// trace:TASK-806 | ai:claude
#[test]
fn latest_burndown_log_picks_newest_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let logdir = root.join(".aida").join("burndown");
    std::fs::create_dir_all(&logdir).unwrap();
    std::fs::write(logdir.join("20260613T100000Z-aaaa1111.jsonl"), "").unwrap();
    std::fs::write(logdir.join("20260613T120000Z-bbbb2222.jsonl"), "").unwrap();
    // A non-jsonl file must be ignored.
    std::fs::write(logdir.join("notes.txt"), "").unwrap();
    let got = latest_burndown_log(root).unwrap();
    assert_eq!(got.file_name().unwrap(), "20260613T120000Z-bbbb2222.jsonl");
}

// trace:TASK-806 | ai:claude
#[test]
fn latest_burndown_log_none_when_dir_absent() {
    let dir = tempfile::tempdir().unwrap();
    assert!(latest_burndown_log(dir.path()).is_none());
}

// ── running-drain marking (TASK-805) ──

fn overlay_with(in_flight: &[&str]) -> DrainOverlay {
    DrainOverlay {
        pid: 4242,
        in_flight: in_flight.iter().map(|s| s.to_string()).collect(),
    }
}

// trace:TASK-805 | ai:claude
#[test]
fn overlay_partition_splits_leased_from_scheduled() {
    let o = overlay_with(&["TASK-2"]);
    let specs = vec![
        "TASK-1".to_string(),
        "TASK-2".to_string(),
        "TASK-3".to_string(),
    ];
    let (in_flight, scheduled) = o.partition(&specs);
    assert_eq!(in_flight, vec!["TASK-2"]);
    // Scheduled is the remainder, sorted.
    assert_eq!(scheduled, vec!["TASK-1", "TASK-3"]);
}

// trace:TASK-805 | ai:claude
#[test]
fn overlay_partition_all_scheduled_when_nothing_leased() {
    let o = overlay_with(&[]);
    let specs = vec!["TASK-3".to_string(), "TASK-1".to_string()];
    let (in_flight, scheduled) = o.partition(&specs);
    assert!(in_flight.is_empty());
    assert_eq!(scheduled, vec!["TASK-1", "TASK-3"]); // sorted
}

// trace:TASK-805 | ai:claude
#[test]
fn drain_banner_names_pid_and_both_buckets() {
    let banner = drain_running_banner(
        4242,
        &["TASK-2".to_string()],
        &["TASK-1".to_string(), "TASK-3".to_string()],
    );
    assert!(banner.contains("pid 4242"), "banner: {banner}");
    assert!(banner.contains("in-flight: TASK-2"), "banner: {banner}");
    assert!(
        banner.contains("scheduled: TASK-1, TASK-3"),
        "banner: {banner}"
    );
}

// trace:TASK-805 | ai:claude
#[test]
fn drain_banner_says_none_for_empty_buckets() {
    let banner = drain_running_banner(7, &[], &["TASK-1".to_string()]);
    assert!(banner.contains("in-flight: none"), "banner: {banner}");
    assert!(banner.contains("scheduled: TASK-1"), "banner: {banner}");
}

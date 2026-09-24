//! STORY-1463: `aida init` offers to install the scheduler tick, and
//! `aida doctor` flags a repo whose scheduler nothing drives.
//!
//! Covers:
//!  - the crontab line/marker builders and the pure install/uninstall body
//!    transforms (no real `crontab` process is ever invoked from a test —
//!    those functions are I/O seams, exercised manually / in real runs only)
//!  - the pure doctor-finding assembly (`build_scheduler_driver_findings`)
//!  - the evidence gatherers (`enabled_substrate_job_count`,
//!    `overdue_substrate_jobs`) against a real `.aida/config.toml`
//!  - CLI parsing for `schedule install-cron` / `uninstall-cron` and
//!    `init --no-schedule`
//!
//! trace:STORY-1463 | ai:claude

use crate::cli::{Cli, Command, MaintenanceScheduleCommand};
use crate::maintenance_schedule::{
    build_scheduler_driver_findings, build_tick_cron_line, classify_cron_driver,
    crontab_after_install, crontab_after_uninstall, enabled_substrate_job_count,
    overdue_substrate_jobs, tick_cron_marker, CronDriverStatus, OverdueSubstrateJob,
};
use chrono::{Duration, TimeZone, Utc};
use clap::Parser;
use std::path::Path;

// ---------------------------------------------------------------------------
// CLI parsing
// ---------------------------------------------------------------------------

#[test]
fn schedule_install_cron_and_uninstall_cron_parse() {
    let cli = Cli::try_parse_from(["aida", "schedule", "install-cron"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Schedule(MaintenanceScheduleCommand::InstallCron)
    ));

    let cli = Cli::try_parse_from(["aida", "schedule", "uninstall-cron"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Schedule(MaintenanceScheduleCommand::UninstallCron)
    ));

    // The `aida cron` visible alias still reaches the same subcommands.
    let cli = Cli::try_parse_from(["aida", "cron", "install-cron"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Schedule(MaintenanceScheduleCommand::InstallCron)
    ));
}

// BUG-1600: the per-turn hook invokes `schedule tick --hook`; the installed
// crontab entry invokes plain `schedule tick` (tagged with
// `AIDA_SCHEDULE_INVOKER=cron`, not a CLI flag). Both shapes must keep
// parsing exactly as before.
// trace:BUG-1600 | ai:claude
#[test]
fn schedule_tick_hook_and_timer_shapes_parse() {
    let cli = Cli::try_parse_from(["aida", "schedule", "tick", "--hook"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Schedule(MaintenanceScheduleCommand::Tick { hook: true })
    ));

    // The timer/cron shape: no --hook.
    let cli = Cli::try_parse_from(["aida", "schedule", "tick"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Schedule(MaintenanceScheduleCommand::Tick { hook: false })
    ));
}

// BUG-1600: `schedule tick` has no JSON projection. This locks in the
// exact clap-introspection fact `enforce_json_format_capability` relies on
// to reject `--format json` before dispatch — if this ever flips (a real
// `--json` gets added to `Tick`), the cron-line builder's choice to pass
// plain `schedule tick` needs a conscious second look, not silent drift.
// trace:BUG-1600 | ai:claude
#[test]
fn schedule_tick_has_no_json_capability() {
    use clap::CommandFactory;
    let root = Cli::command();
    let schedule = root
        .get_subcommands()
        .find(|c| c.get_name() == "schedule")
        .expect("schedule subcommand must exist");
    let tick = schedule
        .get_subcommands()
        .find(|c| c.get_name() == "tick")
        .expect("schedule tick subcommand must exist");
    assert!(
        !tick.get_arguments().any(|a| a.get_id().as_str() == "json"),
        "schedule tick must not declare --json — nothing may pass it --format json"
    );
}

#[test]
fn init_no_schedule_flag_parses() {
    let cli = Cli::try_parse_from(["aida", "init", "--no-schedule"]).unwrap();
    let Command::Init { no_schedule, .. } = cli.command else {
        panic!("expected Command::Init");
    };
    assert!(no_schedule);

    // Default (no flag) is false — a bare `aida init` still only OFFERS at a
    // TTY; the flag exists to skip the offer entirely.
    let cli = Cli::try_parse_from(["aida", "init"]).unwrap();
    let Command::Init { no_schedule, .. } = cli.command else {
        panic!("expected Command::Init");
    };
    assert!(!no_schedule);
}

// ---------------------------------------------------------------------------
// Cron line / marker shape
// ---------------------------------------------------------------------------

#[test]
fn tick_cron_marker_is_keyed_by_repo_path() {
    // A path that (almost certainly) doesn't exist falls back to itself
    // rather than panicking on a failed canonicalize.
    let a = tick_cron_marker(Path::new("/nonexistent/repo/a"));
    let b = tick_cron_marker(Path::new("/nonexistent/repo/b"));
    assert_ne!(a, b, "different repos must get different markers");
    assert!(a.starts_with("aida-schedule-tick:"));
}

#[test]
fn build_tick_cron_line_matches_reference_shape() {
    let repo = Path::new("/home/joe/ai/aida");
    let aida_exe = Path::new("/home/joe/.aida/bin/aida");
    let line = build_tick_cron_line(repo, aida_exe).unwrap();

    // */15 * * * * cd <repo> && PATH='<bin-dir>:...' AIDA_SCHEDULE_INVOKER=cron
    // <abs-aida> schedule tick >> ~/.aida/schedule-tick.log 2>&1 # <marker>
    assert!(line.starts_with("*/15 * * * * cd "), "{line}");
    // The whole PATH assignment is quoted as one shell word (not just the
    // bin dir) so it can never be split or glob-expanded on the way in.
    assert!(
        line.contains("PATH='/home/joe/.aida/bin:/usr/local/bin:/usr/bin:/bin'"),
        "{line}"
    );
    // BUG-1600: tags the invocation source in scheduler telemetry so a
    // cron-driven tick is distinguishable from the per-turn hook (--hook).
    assert!(line.contains("AIDA_SCHEDULE_INVOKER=cron"), "{line}");
    assert!(
        line.contains("'/home/joe/.aida/bin/aida' schedule tick"),
        "{line}"
    );
    // BUG-1600: `schedule tick` has no `--format`/`--json` projection — a
    // prior version of this line added `--format json`, which `aida`
    // rejects before dispatch on every single tick (see the audit in
    // docs/cli-format-json-audit.md). The line must never reintroduce it.
    assert!(
        !line.contains("--format"),
        "must not pass an unsupported --format to `schedule tick`: {line}"
    );
    assert!(
        line.contains("schedule tick >> ~/.aida/schedule-tick.log 2>&1"),
        "{line}"
    );
    assert!(
        line.ends_with(&format!("# {}", tick_cron_marker(repo))),
        "{line}"
    );
}

#[test]
fn build_tick_cron_line_rejects_a_percent_in_any_path_component() {
    // crontab(5): an unescaped `%` in the command field becomes a literal
    // newline (+ stdin redirection) — cron's own parser, before /bin/sh
    // ever runs, regardless of shell quoting. Reject outright rather than
    // silently write a corrupted entry.
    let bad_repo = Path::new("/home/joe/ai/100%-done");
    let aida_exe = Path::new("/home/joe/.aida/bin/aida");
    let err = build_tick_cron_line(bad_repo, aida_exe).unwrap_err();
    assert!(err.to_string().contains('%'), "{err}");

    let good_repo = Path::new("/home/joe/ai/aida");
    let bad_exe = Path::new("/home/joe/.aida%/bin/aida");
    let err = build_tick_cron_line(good_repo, bad_exe).unwrap_err();
    assert!(err.to_string().contains('%'), "{err}");

    // A clean pair still builds fine.
    assert!(build_tick_cron_line(good_repo, aida_exe).is_ok());
}

// ---------------------------------------------------------------------------
// Pure crontab-body transforms (no process ever spawned)
// ---------------------------------------------------------------------------

#[test]
fn crontab_after_install_appends_without_clobbering_existing_entries() {
    let existing = "0 4 * * * some-other-cronjob\n";
    let line = "*/15 * * * * cd /repo && aida schedule tick # aida-schedule-tick:/repo";
    let body = crontab_after_install(existing, "aida-schedule-tick:/repo", line).unwrap();
    assert!(body.starts_with(existing));
    assert!(body.ends_with(&format!("{line}\n")));
}

#[test]
fn crontab_after_install_is_idempotent_when_content_matches() {
    let marker = "aida-schedule-tick:/repo";
    let line = format!("*/15 * * * * cd /repo && aida schedule tick # {marker}");
    let existing = format!("{line}\n");
    assert_eq!(
        crontab_after_install(&existing, marker, &line),
        None,
        "byte-identical entry already installed → true no-op"
    );
}

// BUG-1600: a stale entry (e.g. one installed before this fix, still
// carrying `--format json`) must be rewritten IN PLACE the next time
// install/refresh runs — not silently left broken because "a line with
// this marker already exists".
#[test]
fn crontab_after_install_repairs_a_stale_line_in_place() {
    let marker = "aida-schedule-tick:/repo";
    let stale = format!(
        "*/15 * * * * cd /repo && aida schedule tick --format json >> ~/.aida/schedule-tick.log 2>&1 # {marker}"
    );
    let fresh = format!(
        "*/15 * * * * cd /repo && aida schedule tick >> ~/.aida/schedule-tick.log 2>&1 # {marker}"
    );
    let existing = format!("0 4 * * * some-other-cronjob\n{stale}\n0 5 * * * another-cronjob\n");

    let body = crontab_after_install(&existing, marker, &fresh)
        .expect("stale content must be rewritten, not treated as a no-op");

    assert!(
        !body.contains(&stale),
        "the stale line must be gone: {body}"
    );
    assert!(
        body.contains(&fresh),
        "the fresh line must replace it: {body}"
    );
    // Repaired in place, not appended at the end — every other line's
    // relative order is preserved.
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines[0], "0 4 * * * some-other-cronjob");
    assert_eq!(lines[1], fresh);
    assert_eq!(lines[2], "0 5 * * * another-cronjob");
}

#[test]
fn crontab_after_install_handles_no_trailing_newline() {
    let existing = "0 4 * * * some-other-cronjob"; // no trailing newline
    let line = "*/15 * * * * cd /repo && aida schedule tick # aida-schedule-tick:/repo";
    let body = crontab_after_install(existing, "aida-schedule-tick:/repo", line).unwrap();
    assert_eq!(
        body,
        "0 4 * * * some-other-cronjob\n*/15 * * * * cd /repo && aida schedule tick # aida-schedule-tick:/repo\n"
    );
}

#[test]
fn crontab_after_uninstall_removes_only_the_matching_repo_line() {
    let marker_a = "aida-schedule-tick:/repo/a";
    let marker_b = "aida-schedule-tick:/repo/b";
    let existing = format!(
        "0 4 * * * unrelated\n*/15 * * * * cd /repo/a && aida schedule tick # {marker_a}\n*/15 * * * * cd /repo/b && aida schedule tick # {marker_b}\n"
    );
    let body = crontab_after_uninstall(&existing, marker_a).unwrap();
    assert!(!body.contains(marker_a));
    assert!(
        body.contains(marker_b),
        "must not touch a different repo's entry"
    );
    assert!(body.contains("0 4 * * * unrelated"));
}

#[test]
fn crontab_after_uninstall_no_op_when_marker_absent() {
    let existing = "0 4 * * * unrelated\n";
    assert_eq!(
        crontab_after_uninstall(existing, "aida-schedule-tick:/repo"),
        None
    );
}

/// Regression: `/x/aida`'s marker is a textual PREFIX of `/x/aida-web`'s
/// marker (`aida-schedule-tick:/x/aida` vs `aida-schedule-tick:/x/aida-web`).
/// A naive `.contains(marker)` on `/x/aida-web`'s line would wrongly match
/// while checking `/x/aida`'s marker — install would think it's already
/// installed, and uninstall would delete the WRONG repo's entry. Anchoring
/// on the trailing `# <marker>` token must tell them apart.
#[test]
fn marker_matching_does_not_confuse_a_repo_whose_path_is_a_prefix_of_another() {
    let marker_short = tick_cron_marker(Path::new("/x/aida"));
    let marker_long = tick_cron_marker(Path::new("/x/aida-web"));
    assert_ne!(marker_short, marker_long);
    assert!(
        marker_long.starts_with(&marker_short),
        "the test setup must exercise a genuine textual prefix: {marker_short} / {marker_long}"
    );

    let line_long = format!("*/15 * * * * cd /x/aida-web && aida schedule tick # {marker_long}");
    let existing = format!("{line_long}\n");

    // Installing the SHORT repo's entry must not be short-circuited by the
    // long repo's line already being present.
    let body = crontab_after_install(&existing, &marker_short, "irrelevant-new-line").unwrap();
    assert!(body.contains(&line_long), "must keep the long repo's entry");
    assert!(
        body.contains("irrelevant-new-line"),
        "must actually append the short repo's entry, not treat it as already installed"
    );

    // Uninstalling the SHORT repo's entry must not delete the long repo's
    // line even though it's a textual superset match.
    assert_eq!(
        crontab_after_uninstall(&existing, &marker_short),
        None,
        "the short repo's marker is not present as its own line — must be a no-op, not a false hit on the long repo's line"
    );

    // And uninstalling the LONG repo's entry (which IS present) must still
    // work correctly.
    let body = crontab_after_uninstall(&existing, &marker_long).unwrap();
    assert!(!body.contains(&marker_long));
}

#[test]
fn classify_cron_driver_does_not_confuse_a_repo_whose_path_is_a_prefix_of_another() {
    let marker_short = tick_cron_marker(Path::new("/x/aida"));
    let marker_long = tick_cron_marker(Path::new("/x/aida-web"));
    let body = format!("*/15 * * * * cd /x/aida-web && aida schedule tick # {marker_long}\n");

    // Only the long repo's driver is actually installed.
    assert_eq!(
        classify_cron_driver(Ok(Some(body.clone())), &marker_long),
        CronDriverStatus::Installed
    );
    assert_eq!(
        classify_cron_driver(Ok(Some(body)), &marker_short),
        CronDriverStatus::Missing,
        "must not report the short repo's driver as installed off a prefix match on the long repo's line"
    );
}

// ---------------------------------------------------------------------------
// Driver-status classification (PRIN-5: unavailable evidence → unknown, not ok)
// ---------------------------------------------------------------------------

#[test]
fn classify_cron_driver_installed_missing_unknown() {
    let marker = "aida-schedule-tick:/repo";
    assert_eq!(
        classify_cron_driver(Ok(Some(format!("... # {marker}\n"))), marker),
        CronDriverStatus::Installed
    );
    assert_eq!(
        classify_cron_driver(Ok(Some("0 4 * * * unrelated\n".to_string())), marker),
        CronDriverStatus::Missing
    );
    assert_eq!(
        classify_cron_driver(Ok(None), marker),
        CronDriverStatus::Missing,
        "no crontab at all for this user is still Missing, not Unknown"
    );
    assert_eq!(
        classify_cron_driver(
            Err("`crontab` is not installed or not on PATH".to_string()),
            marker
        ),
        CronDriverStatus::Unknown("`crontab` is not installed or not on PATH".to_string())
    );
}

// ---------------------------------------------------------------------------
// Evidence gatherers against a real `.aida/config.toml`
// ---------------------------------------------------------------------------

fn write_schedule_config(project_root: &Path, body: &str) {
    std::fs::create_dir_all(project_root.join(".aida")).unwrap();
    std::fs::write(project_root.join(".aida/config.toml"), body).unwrap();
}

#[test]
fn enabled_substrate_job_count_counts_only_enabled_substrate_jobs() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::test_env::env_lock();
    std::env::set_var("AIDA_HOME", tmp.path());
    write_schedule_config(
        tmp.path(),
        r#"
[schedule]
[[schedule.jobs]]
name = "enabled-substrate"
command = "queue gc"
every = "6h"
enabled = true

[[schedule.jobs]]
name = "disabled-substrate"
command = "cache verify"
every = "24h"
enabled = false

[[schedule.jobs]]
name = "enabled-seat"
seats = ["advisor"]
prompt = "triage"
every = "30m"
enabled = true
"#,
    );
    let count = enabled_substrate_job_count(tmp.path()).unwrap();
    std::env::remove_var("AIDA_HOME");
    assert_eq!(count, 1);
}

#[test]
fn enabled_substrate_job_count_is_zero_with_no_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::test_env::env_lock();
    std::env::set_var("AIDA_HOME", tmp.path());
    let count = enabled_substrate_job_count(tmp.path()).unwrap();
    std::env::remove_var("AIDA_HOME");
    assert_eq!(count, 0);
}

#[test]
fn overdue_substrate_jobs_flags_only_jobs_past_2x_interval() {
    let tmp = tempfile::tempdir().unwrap();
    let _guard = crate::test_env::env_lock();
    std::env::set_var("AIDA_HOME", tmp.path());
    write_schedule_config(
        tmp.path(),
        r#"
[schedule]
[[schedule.jobs]]
name = "way-overdue"
command = "queue gc"
every = "1h"
enabled = true

[[schedule.jobs]]
name = "on-time"
command = "cache verify"
every = "1h"
enabled = true

[[schedule.jobs]]
name = "never-run"
command = "doctor"
every = "1h"
enabled = true
"#,
    );
    // Seed local run state directly as JSON (the `.aida/schedule-state.json`
    // on-disk shape) rather than reaching into `maintenance_schedule`'s
    // private `ScheduleState` — this is the same file `aida schedule tick`
    // writes. "way-overdue" ran 3h ago (> 2x its 1h interval), "on-time" ran
    // 10m ago (well under 2x), "never-run" has no recorded run at all.
    std::fs::write(
        tmp.path().join(".aida/schedule-state.json"),
        format!(
            r#"{{"tasks":{{"way-overdue":{{"last_run_at":"{}"}},"on-time":{{"last_run_at":"{}"}}}}}}"#,
            (Utc::now() - Duration::hours(3)).to_rfc3339(),
            (Utc::now() - Duration::minutes(10)).to_rfc3339(),
        ),
    )
    .unwrap();

    let overdue = overdue_substrate_jobs(tmp.path()).unwrap();
    std::env::remove_var("AIDA_HOME");

    let names: Vec<&str> = overdue.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, vec!["way-overdue"]);
}

// ---------------------------------------------------------------------------
// Pure finding assembly (PRIN-5: unknown evidence must never render as ok)
// ---------------------------------------------------------------------------

fn at(hour: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 22, hour, 0, 0).unwrap()
}

#[test]
fn no_finding_when_no_enabled_substrate_jobs() {
    let findings = build_scheduler_driver_findings(0, CronDriverStatus::Missing, &[], at(12));
    assert!(findings.is_empty());
}

#[test]
fn no_finding_when_driver_installed_and_nothing_overdue() {
    let findings = build_scheduler_driver_findings(2, CronDriverStatus::Installed, &[], at(12));
    assert!(findings.is_empty());
}

#[test]
fn finding_when_enabled_jobs_but_no_driver_installed() {
    let findings = build_scheduler_driver_findings(2, CronDriverStatus::Missing, &[], at(12));
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].category, "scheduler-driver");
    assert_eq!(findings[0].id, "scheduler-tick-not-installed");
    assert!(findings[0].summary.contains('2'));
    assert_eq!(findings[0].action, "aida schedule install-cron");
    assert!(!findings[0].safe_heal);
}

#[test]
fn finding_says_unknown_not_ok_when_driver_status_cannot_be_determined() {
    let findings = build_scheduler_driver_findings(
        1,
        CronDriverStatus::Unknown("no crontab on Windows".to_string()),
        &[],
        at(12),
    );
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "scheduler-tick-driver-unknown");
    assert!(findings[0].summary.contains("unknown"));
    assert!(
        !findings[0].summary.contains("status: ok")
            && !findings[0].summary.trim_end().ends_with(", ok"),
        "PRIN-5: unavailable evidence must never be reported as ok: {}",
        findings[0].summary
    );
}

#[test]
fn finding_when_a_substrate_job_is_overdue_even_with_driver_installed() {
    let overdue = vec![OverdueSubstrateJob {
        name: "performance-guard".to_string(),
        interval: Duration::hours(6),
        last_run: at(12) - Duration::hours(20),
    }];
    let findings =
        build_scheduler_driver_findings(1, CronDriverStatus::Installed, &overdue, at(12));
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "scheduler-job-overdue");
    assert!(findings[0].summary.contains("performance-guard"));
    assert!(!findings[0].safe_heal);
}

#[test]
fn both_findings_can_fire_together() {
    let overdue = vec![OverdueSubstrateJob {
        name: "performance-guard".to_string(),
        interval: Duration::hours(6),
        last_run: at(12) - Duration::hours(20),
    }];
    let findings = build_scheduler_driver_findings(1, CronDriverStatus::Missing, &overdue, at(12));
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["scheduler-tick-not-installed", "scheduler-job-overdue"]
    );
}

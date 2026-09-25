//! TASK-1491 (STORY-1218 slice 2): the systemd user-timer driver, the
//! driver-switch rules (advisor A12/A13) and the systemd-aware doctor.
//!
//! No test here runs the real `systemctl --user`, reads or writes the real
//! crontab, or touches the real `~/.config/systemd`: every side effect goes
//! through [`FakeHost`] (an in-memory crontab, a recording fake `systemctl`,
//! and a unit directory under a temporary fake HOME). The real host itself
//! refuses those calls under `cfg(test)`, which the last tests pin.
//!
//! trace:TASK-1491 | ai:claude

use super::*;
use crate::cli::{Cli, Command, MaintenanceScheduleCommand, ShiftCommand};
use crate::maintenance_schedule::{
    build_scheduler_driver_findings, build_tick_cron_line, crontab_after_driver_switch,
    init_tick_offer_allowed, render_tick_cron_line, scheduler_driver_check_needed, tick_invocation,
    CronDriverStatus, DriverInstallOutcome,
};
use clap::Parser;
use std::collections::BTreeSet;

const REPO: &str = "/home/op/ai/aida";
const EXE: &str = "/home/op/.local/bin/aida";

fn marker() -> String {
    format!("aida-schedule-tick:{REPO}")
}

fn inv() -> TickInvocation {
    tick_invocation(Path::new(REPO), Path::new(EXE)).unwrap()
}

/// In-memory crontab, recording fake `systemctl --user`, fake-HOME unit dir.
struct FakeHost {
    _home: tempfile::TempDir,
    unit_dir: PathBuf,
    crontab: Option<String>,
    crontab_read_err: bool,
    crontab_write_err: bool,
    /// `crontab -` "succeeds" but the body never lands (read-back check).
    crontab_drops_writes: bool,
    enabled: BTreeSet<String>,
    /// `enable` "succeeds" but `is-enabled` keeps saying disabled.
    enable_does_not_stick: bool,
    /// `enable` fails (e.g. the user manager reads another unit directory).
    enable_fails: bool,
    /// Timers started (`start` / `restart`), for `is-active`.
    started: BTreeSet<String>,
    /// No user bus: every call exits 1 with nothing on stdout.
    no_bus: bool,
    /// `start` / `restart` fail (BUG-1619).
    start_fails: bool,
    /// `disable --now` fails (BUG-1619).
    disable_fails: bool,
    /// Whether the unit file was still on disk at each `disable --now`, to
    /// pin "disable first, then remove" (BUG-1619).
    unit_present_at_disable: Vec<bool>,
    /// `is-enabled` state for a timer whose file is not in `unit_dir`: a
    /// copy systemd loads from another directory (BUG-1619).
    elsewhere: Option<&'static str>,
    /// Old systemd: an unknown unit prints nothing on stdout and a "No such
    /// file or directory" error, instead of `not-found` (BUG-1619).
    legacy_not_found: bool,
    log: Vec<Vec<String>>,
    linger: Option<bool>,
    supported: bool,
}

impl FakeHost {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let unit_dir = home.path().join(".config").join("systemd").join("user");
        FakeHost {
            _home: home,
            unit_dir,
            crontab: None,
            crontab_read_err: false,
            crontab_write_err: false,
            crontab_drops_writes: false,
            enabled: BTreeSet::new(),
            enable_does_not_stick: false,
            enable_fails: false,
            started: BTreeSet::new(),
            no_bus: false,
            start_fails: false,
            disable_fails: false,
            unit_present_at_disable: Vec::new(),
            elsewhere: None,
            legacy_not_found: false,
            log: Vec::new(),
            linger: Some(true),
            supported: true,
        }
    }

    fn unit(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.unit_dir.join(name)).ok()
    }

    fn put_unit(&self, name: &str, body: &str) {
        std::fs::create_dir_all(&self.unit_dir).unwrap();
        std::fs::write(self.unit_dir.join(name), body).unwrap();
    }

    fn calls(&self) -> Vec<String> {
        self.log.iter().map(|c| c.join(" ")).collect()
    }
}

impl DriverHost for FakeHost {
    fn read_crontab(&mut self) -> Result<Option<String>> {
        if self.crontab_read_err {
            anyhow::bail!("`crontab` is not installed or not on PATH");
        }
        Ok(self.crontab.clone())
    }

    fn write_crontab(&mut self, body: &str) -> Result<()> {
        if self.crontab_write_err {
            anyhow::bail!("`crontab -` exited with 1");
        }
        if !self.crontab_drops_writes {
            self.crontab = Some(body.to_string());
        }
        Ok(())
    }

    fn systemctl_user(&mut self, args: &[&str]) -> Result<CommandOutput> {
        self.log.push(args.iter().map(|a| a.to_string()).collect());
        let ok = |stdout: &str| CommandOutput {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        };
        if self.no_bus {
            return Ok(CommandOutput {
                success: false,
                stdout: String::new(),
                // The real systemd 255 no-user-bus error. trace:BUG-1619 | ai:claude
                stderr: "Failed to connect to bus: No such file or directory\n".to_string(),
            });
        }
        Ok(match args {
            ["enable", _] if self.enable_fails => CommandOutput {
                success: false,
                stdout: String::new(),
                stderr: "Failed to enable unit: Unit file does not exist.\n".to_string(),
            },
            ["start" | "restart", _] if self.start_fails => CommandOutput {
                success: false,
                stdout: String::new(),
                stderr: "Job for the timer failed.\n".to_string(),
            },
            ["disable", "--now", unit] if self.disable_fails => {
                self.unit_present_at_disable
                    .push(self.unit_dir.join(unit).exists());
                CommandOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: "Failed to disable unit.\n".to_string(),
                }
            }
            ["start" | "restart", unit] => {
                self.started.insert(unit.to_string());
                ok("")
            }
            ["is-active", unit] => {
                if self.started.contains(*unit) {
                    ok("active\n")
                } else {
                    CommandOutput {
                        success: false,
                        stdout: "inactive\n".to_string(),
                        stderr: String::new(),
                    }
                }
            }
            ["enable", unit] => {
                if !self.enable_does_not_stick {
                    self.enabled.insert(unit.to_string());
                }
                ok("")
            }
            ["disable", "--now", unit] => {
                self.unit_present_at_disable
                    .push(self.unit_dir.join(unit).exists());
                self.enabled.remove(*unit);
                self.started.remove(*unit);
                ok("")
            }
            // trace:BUG-1619 | ai:claude
            ["is-enabled", unit] => {
                let not_enabled = |stdout: &str| CommandOutput {
                    success: false,
                    stdout: format!("{stdout}\n"),
                    stderr: String::new(),
                };
                if self.enabled.contains(*unit) {
                    ok("enabled\n")
                } else if self.unit_dir.join(unit).exists() {
                    not_enabled("disabled")
                } else if let Some(state) = self.elsewhere {
                    not_enabled(state)
                } else if self.legacy_not_found {
                    CommandOutput {
                        success: false,
                        stdout: String::new(),
                        stderr: format!(
                            "Failed to get unit file state for {unit}: No such file or directory\n"
                        ),
                    }
                } else {
                    not_enabled("not-found")
                }
            }
            _ => ok(""),
        })
    }

    fn unit_dir(&self) -> Result<PathBuf> {
        Ok(self.unit_dir.clone())
    }

    fn linger_enabled(&mut self) -> Option<bool> {
        self.linger
    }

    fn systemd_supported(&self) -> bool {
        self.supported
    }
}

fn names() -> (String, String) {
    unit_names(REPO)
}

// ---------------------------------------------------------------------------
// A12: unit shape, shared invocation
// ---------------------------------------------------------------------------

#[test]
fn systemd_units_share_tick_invocation_marker_abs_exe_no_format_flag() {
    let inv = inv();
    let units = build_systemd_units(&inv);
    let cron = build_tick_cron_line(Path::new(REPO), Path::new(EXE)).unwrap();
    assert_eq!(cron, render_tick_cron_line(&inv), "one builder for cron");

    // Same argv: absolute binary + exactly `schedule tick`, no format flag.
    assert!(units
        .service
        .contains(&format!("\nExecStart=\"{EXE}\" schedule tick\n")));
    assert!(cron.contains(&format!("'{EXE}' schedule tick >>")));
    for body in [&units.service, &units.timer, &cron] {
        assert!(!body.contains("--format"), "{body}");
        assert!(!body.contains("--json"), "{body}");
    }
    // Same PATH, same working directory, same marker; per-driver invoker tag.
    assert!(units.service.contains(&format!(
        "Environment=\"PATH={}\" \"AIDA_SCHEDULE_INVOKER=systemd\"",
        inv.path_env
    )));
    assert!(cron.contains(&format!(
        "PATH='{}' AIDA_SCHEDULE_INVOKER=cron",
        inv.path_env
    )));
    assert!(units
        .service
        .contains(&format!("\nWorkingDirectory={REPO}\n")));
    assert!(cron.contains(&format!("cd '{REPO}' &&")));
    assert!(unit_has_marker(&units.service, &marker()));
    assert!(unit_has_marker(&units.timer, &marker()));
    assert!(cron.ends_with(&format!("# {}", marker())));
    // Unit names are keyed by a stable hash of the canonical repo path.
    let (service, timer) = names();
    assert_eq!(units.service_name, service);
    assert_eq!(units.timer_name, timer);
    assert!(service.starts_with("aida-tick-") && service.len() == "aida-tick-".len() + 8 + 8);
    assert_ne!(unit_stem(REPO), unit_stem("/home/op/ai/aida-web"));
    assert!(units.timer.contains(&format!("\nUnit={service}\n")));
}

#[test]
fn systemd_unit_has_killmode_process_and_first_fire_trigger() {
    let units = build_systemd_units(&inv());
    let svc = &units.service;
    let tmr = &units.timer;
    assert!(svc.contains("\nType=oneshot\n"));
    assert!(
        svc.contains("\nKillMode=process\n"),
        "A12: KillMode=process"
    );
    assert!(
        svc.contains("\nTimeoutStartSec=15min\n"),
        "A12: start timeout"
    );
    assert!(svc.contains("\nStandardOutput=journal\n"));
    assert!(
        tmr.contains("\nOnActiveSec=1min\n"),
        "A12: first-fire trigger"
    );
    assert!(
        tmr.contains("\nOnBootSec=2min\n"),
        "A12: first-fire after boot"
    );
    assert!(tmr.contains("\nOnUnitInactiveSec=10min\n"));
    assert!(tmr.contains("\nWantedBy=timers.target\n"));
    assert!(!tmr.contains("Persistent"), "A12: Persistent dropped");
    for limit in [
        "MemoryMax",
        "MemoryHigh",
        "MemoryLimit",
        "CPUQuota",
        "CPUWeight",
        "TasksMax",
        "IOWeight",
        "LimitNOFILE",
        "RuntimeMaxSec",
    ] {
        assert!(!svc.contains(limit), "A12: no resource limit ({limit})");
        assert!(!tmr.contains(limit), "A12: no resource limit ({limit})");
    }
}

#[test]
fn systemd_units_quote_awkward_paths_and_refuse_specifiers() {
    let repo = Path::new("/home/op/my repo");
    let exe = Path::new("/opt/a\"b$c/aida");
    let units = build_systemd_units(&tick_invocation(repo, exe).unwrap());
    assert!(units
        .service
        .contains("ExecStart=\"/opt/a\\\"b$$c/aida\" schedule tick"));
    assert!(units
        .service
        .contains("WorkingDirectory=/home/op/my repo\n"));
    // `%` is a systemd specifier and a cron newline; a line break breaks both.
    assert!(tick_invocation(Path::new("/x/100%"), Path::new(EXE)).is_err());
    assert!(tick_invocation(Path::new("/x/a\nb"), Path::new(EXE)).is_err());
}

#[test]
fn unit_marker_match_is_whole_line_not_prefix() {
    let sibling = build_systemd_units(
        &tick_invocation(Path::new("/home/op/ai/aida-web"), Path::new(EXE)).unwrap(),
    );
    assert!(!unit_has_marker(&sibling.service, &marker()));
    assert!(!unit_has_marker(&format!("## {}\n", marker()), &marker()));
}

// ---------------------------------------------------------------------------
// A13: systemd install (compare-and-rewrite, verify, refuse foreign)
// ---------------------------------------------------------------------------

#[test]
fn systemd_install_installed_uptodate_repaired() {
    let mut host = FakeHost::new();
    let inv = inv();
    let (service, timer) = names();

    let r = switch_driver(&mut host, &inv, Driver::Systemd).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::Installed);
    assert!(host.unit(&service).is_some() && host.unit(&timer).is_some());
    assert!(host.enabled.contains(&timer));
    assert!(host.calls().contains(&"daemon-reload".to_string()));
    assert!(host.calls().contains(&format!("is-enabled {timer}")));

    host.log.clear();
    let r = switch_driver(&mut host, &inv, Driver::Systemd).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::AlreadyUpToDate);
    assert!(
        !host.calls().contains(&"daemon-reload".to_string()),
        "no rewrite, no reload"
    );

    // An older unit of ours (e.g. with the dropped Persistent=true) is
    // rewritten in place.
    let old = host.unit(&timer).unwrap().replace(
        "OnUnitInactiveSec=10min\n",
        "OnUnitInactiveSec=10min\nPersistent=true\n",
    );
    host.put_unit(&timer, &old);
    host.log.clear();
    let r = switch_driver(&mut host, &inv, Driver::Systemd).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::Repaired);
    assert!(!host.unit(&timer).unwrap().contains("Persistent"));
    assert!(host.calls().contains(&format!("restart {timer}")));
}

#[test]
fn systemd_install_refuses_to_overwrite_a_unit_without_our_marker() {
    let mut host = FakeHost::new();
    let (service, _) = names();
    let foreign = "[Service]\nExecStart=/usr/bin/true\n";
    host.put_unit(&service, foreign);
    host.crontab = Some(format!(
        "{}\n",
        build_tick_cron_line(Path::new(REPO), Path::new(EXE)).unwrap()
    ));
    let before = host.crontab.clone();

    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    assert!(err.to_string().contains("Refusing to overwrite"), "{err}");
    assert_eq!(host.unit(&service).as_deref(), Some(foreign), "untouched");
    assert!(host.log.is_empty(), "no systemctl call after a refusal");
    assert_eq!(
        host.crontab, before,
        "the old driver stays when the new one fails"
    );
}

#[test]
fn systemd_install_verifies_before_removing_cron() {
    let mut host = FakeHost::new();
    host.enable_does_not_stick = true;
    let cron = build_tick_cron_line(Path::new(REPO), Path::new(EXE)).unwrap();
    host.crontab = Some(format!("{cron}\n"));

    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    assert!(format!("{err:#}").contains("is-enabled"), "{err:#}");
    assert_eq!(
        host.crontab.as_deref(),
        Some(format!("{cron}\n").as_str()),
        "A13a: never none — cron kept when the timer did not verify"
    );
}

#[test]
fn systemd_install_removes_this_repos_cron_incl_legacy_unmarked_only() {
    let mut host = FakeHost::new();
    let marked = build_tick_cron_line(Path::new(REPO), Path::new(EXE)).unwrap();
    let legacy = format!(
        "*/15 * * * * cd {REPO} && PATH=/usr/bin {REPO}/target/release/aida schedule tick --format json >> ~/.aida/schedule-tick.log 2>&1"
    );
    let sibling = build_tick_cron_line(Path::new("/home/op/ai/aida-web"), Path::new(EXE)).unwrap();
    let sibling_legacy =
        "*/15 * * * * cd /home/op/ai/aida-web && aida schedule tick >> ~/.aida/schedule-tick.log 2>&1";
    let commented = format!("# {legacy}");
    let unrelated = "0 4 * * * backup.sh";
    let body = format!("{unrelated}\n{marked}\n{sibling}\n{legacy}\n{commented}\n{sibling_legacy}");
    host.crontab = Some(body);

    let r = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    assert_eq!(r.removed_cron_lines, vec![marked.clone(), legacy.clone()]);
    assert_eq!(
        host.crontab.as_deref().unwrap(),
        format!("{unrelated}\n{sibling}\n{commented}\n{sibling_legacy}"),
        "everything else byte-for-byte, missing final newline kept"
    );
}

#[test]
fn systemd_install_with_unreadable_crontab_warns_and_keeps_timer() {
    let mut host = FakeHost::new();
    host.crontab_read_err = true;
    let r = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    assert_eq!(r.warnings.len(), 1);
    assert!(host.enabled.contains(&names().1));
}

#[test]
fn systemd_install_crontab_write_failure_names_both_drivers() {
    let mut host = FakeHost::new();
    host.crontab = Some(format!(
        "{}\n",
        build_tick_cron_line(Path::new(REPO), Path::new(EXE)).unwrap()
    ));
    host.crontab_write_err = true;
    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    assert!(
        format!("{err:#}").contains("both drivers now run"),
        "{err:#}"
    );
    assert!(host.enabled.contains(&names().1), "timer stays installed");
}

#[test]
fn systemd_install_prints_linger_reminder_only_when_linger_is_off() {
    for (linger, want) in [(Some(false), true), (Some(true), false), (None, false)] {
        let mut host = FakeHost::new();
        host.linger = linger;
        let r = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
        assert_eq!(r.linger_off, want, "linger {linger:?}");
    }
}

#[test]
fn systemd_install_refused_on_a_platform_without_systemd() {
    let mut host = FakeHost::new();
    host.supported = false;
    assert!(switch_driver(&mut host, &inv(), Driver::Systemd).is_err());
    assert!(host.log.is_empty());
}

// ---------------------------------------------------------------------------
// A13b: crontab_after_driver_switch
// ---------------------------------------------------------------------------

#[test]
fn crontab_after_driver_switch_touches_only_this_repos_active_lines() {
    let m = marker();
    let marked = format!("*/15 * * * * cd {REPO} && aida schedule tick # {m}");
    let legacy = format!("*/15 * * * * cd '{REPO}' && '{EXE}' schedule tick");
    let prefix_marked = "*/15 * * * * cd /home/op/ai/aida-web && aida schedule tick # aida-schedule-tick:/home/op/ai/aida-web";
    let prefix_legacy = "*/15 * * * * cd /home/op/ai/aida-web && aida schedule tick";
    let commented_marked = format!("#{marked}");
    let commented_legacy = format!("  # {legacy}");
    let unrelated = "30 2 * * * cd /home/op/ai/aida && make backup";
    let existing = format!(
        "{unrelated}\r\n{marked}\n{prefix_marked}\n{commented_marked}\n{legacy}\n{commented_legacy}\n{prefix_legacy}"
    );
    let (body, removed) = crontab_after_driver_switch(&existing, &m).unwrap();
    assert_eq!(removed, vec![marked, legacy]);
    assert_eq!(
        body,
        format!(
            "{unrelated}\r\n{prefix_marked}\n{commented_marked}\n{commented_legacy}\n{prefix_legacy}"
        )
    );
}

#[test]
fn crontab_after_driver_switch_last_line_without_newline() {
    let m = marker();
    let marked = format!("*/15 * * * * cd {REPO} && aida schedule tick # {m}");
    let (body, removed) =
        crontab_after_driver_switch(&format!("0 4 * * * x\n{marked}"), &m).unwrap();
    assert_eq!(body, "0 4 * * * x\n");
    assert_eq!(removed, vec![marked]);
    // Nothing of ours: None, so the caller never rewrites the crontab.
    assert_eq!(crontab_after_driver_switch("0 4 * * * x", &m), None);
    assert_eq!(crontab_after_driver_switch("", &m), None);
}

// ---------------------------------------------------------------------------
// A13a/c: cron install removes this repo's timer after verifying
// ---------------------------------------------------------------------------

#[test]
fn cron_install_disables_timer_never_service_and_deletes_only_marked_units() {
    let mut host = FakeHost::new();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    let (service, timer) = names();
    host.log.clear();

    let r = switch_driver(&mut host, &inv(), Driver::Cron).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::Installed);
    let removal = r.systemd_removal.unwrap();
    assert!(removal.disabled_timer);
    assert_eq!(removal.deleted, vec![timer.clone(), service.clone()]);
    assert_eq!(
        host.calls(),
        vec![
            format!("disable --now {timer}"),
            "daemon-reload".to_string()
        ]
    );
    assert!(
        host.log.iter().flatten().all(|a| !a.ends_with(".service")),
        "A13c: the service is never stopped or disabled"
    );
    assert!(host.unit(&timer).is_none() && host.unit(&service).is_none());
    assert!(host
        .crontab
        .as_deref()
        .unwrap()
        .contains(&format!("# {}", marker())));
}

#[test]
fn cron_install_leaves_units_without_our_marker() {
    let mut host = FakeHost::new();
    let (service, timer) = names();
    host.put_unit(&timer, "[Timer]\nOnBootSec=1h\n");
    host.put_unit(&service, "[Service]\nExecStart=/usr/bin/true\n");

    let r = switch_driver(&mut host, &inv(), Driver::Cron).unwrap();
    let removal = r.systemd_removal.unwrap();
    assert!(!removal.disabled_timer);
    assert!(removal.deleted.is_empty());
    assert_eq!(removal.refused.len(), 2);
    assert!(host.log.is_empty(), "a foreign timer is never disabled");
    assert!(host.unit(&timer).is_some() && host.unit(&service).is_some());
}

#[test]
fn cron_install_that_does_not_read_back_keeps_the_timer() {
    let mut host = FakeHost::new();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    host.crontab_drops_writes = true;
    host.log.clear();

    let err = switch_driver(&mut host, &inv(), Driver::Cron).unwrap_err();
    assert!(
        err.to_string().contains("reading the crontab back"),
        "{err}"
    );
    assert!(host.log.is_empty(), "A13a: timer untouched");
    assert!(host.unit(&names().1).is_some());
}

#[test]
fn cron_install_repairs_legacy_line_and_is_idempotent() {
    let mut host = FakeHost::new();
    let legacy = format!("*/15 * * * * cd {REPO} && aida schedule tick --format json");
    host.crontab = Some(format!("{legacy}\n"));
    let r = switch_driver(&mut host, &inv(), Driver::Cron).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::Repaired);
    let r = switch_driver(&mut host, &inv(), Driver::Cron).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::AlreadyUpToDate);
    assert_eq!(host.crontab.as_deref().unwrap().lines().count(), 1);
}

#[test]
fn systemd_uninstall_is_idempotent_and_marker_gated() {
    let mut host = FakeHost::new();
    let r = remove_systemd_units(&mut host, REPO, &marker()).unwrap();
    assert!(!r.removed_anything());
    assert!(host.log.is_empty());

    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    // A different repo's removal never touches ours.
    let other = remove_systemd_units(
        &mut host,
        "/home/op/ai/aida-web",
        "aida-schedule-tick:/home/op/ai/aida-web",
    )
    .unwrap();
    assert!(!other.removed_anything());
    assert!(host.unit(&names().1).is_some());
}

// ---------------------------------------------------------------------------
// Doctor: DriverStatus { cron, systemd }
// ---------------------------------------------------------------------------

#[test]
fn doctor_driver_status_systemd_and_dual_driver() {
    let m = marker();
    let now = chrono::Utc::now();

    // Both installed (a switch that failed part-way).
    let mut host = FakeHost::new();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    host.crontab = Some(format!(
        "{}\n",
        build_tick_cron_line(Path::new(REPO), Path::new(EXE)).unwrap()
    ));
    let both = driver_status_with(&mut host, REPO, &m);
    assert_eq!(both.cron, CronDriverStatus::Installed);
    assert_eq!(both.systemd, SystemdDriverStatus::Installed);
    assert!(both.both_installed());
    assert!(both.label().contains("both installed"));
    let f = build_scheduler_driver_findings(1, &both, &[], now);
    assert_eq!(
        f.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        vec!["scheduler-tick-dual-driver"]
    );
    // Flagged even with no registered jobs: the pure builder emits it, and
    // doctor looks whenever our timer file is present.
    assert_eq!(build_scheduler_driver_findings(0, &both, &[], now).len(), 1);
    assert!(scheduler_driver_check_needed(0, false, true));
    assert!(!scheduler_driver_check_needed(0, false, false));
    assert!(systemd_timer_file_present(&host, REPO, &m));
    assert!(!systemd_timer_file_present(&FakeHost::new(), REPO, &m));

    // Systemd only: NOT reported as driverless.
    host.crontab = Some("0 4 * * * unrelated\n".to_string());
    let sys = driver_status_with(&mut host, REPO, &m);
    assert_eq!(sys.cron, CronDriverStatus::Missing);
    assert!(sys.systemd_installed() && !sys.both_installed());
    assert!(build_scheduler_driver_findings(2, &sys, &[], now).is_empty());

    // Timer present but disabled: not driving.
    host.enabled.clear();
    let disabled = driver_status_with(&mut host, REPO, &m);
    assert_eq!(disabled.systemd, SystemdDriverStatus::Disabled);
    let f = build_scheduler_driver_findings(2, &disabled, &[], now);
    assert_eq!(f[0].id, "scheduler-tick-not-installed");
    assert!(f[0].summary.contains("disabled"), "{}", f[0].summary);

    // Neither: recommend the systemd driver where it exists.
    let none = driver_status_with(&mut FakeHost::new(), REPO, &m);
    assert_eq!(none.systemd, SystemdDriverStatus::Missing);
    let f = build_scheduler_driver_findings(2, &none, &[], now);
    assert_eq!(f[0].id, "scheduler-tick-not-installed");
    assert_eq!(f[0].action, "aida schedule install-systemd");

    // Unknown cron with no systemd verdict stays unknown, never ok.
    let mut broken = FakeHost::new();
    broken.crontab_read_err = true;
    broken.supported = false;
    let unknown = driver_status_with(&mut broken, REPO, &m);
    assert_eq!(unknown.systemd, SystemdDriverStatus::Unsupported);
    let f = build_scheduler_driver_findings(1, &unknown, &[], now);
    assert_eq!(f[0].id, "scheduler-tick-driver-unknown");
    assert_eq!(f[0].action, "aida schedule install-cron");
}

#[test]
fn doctor_no_user_bus_is_unknown_never_disabled() {
    let m = marker();
    let now = chrono::Utc::now();
    let mut host = FakeHost::new();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    // The session loses its user bus (cron, ssh without pam_systemd, a
    // sandbox): `systemctl --user` prints nothing on stdout and exits 1.
    host.no_bus = true;
    host.crontab = Some("0 4 * * * unrelated\n".to_string());
    let st = driver_status_with(&mut host, REPO, &m);
    match &st.systemd {
        SystemdDriverStatus::Unknown(r) => assert!(r.contains("Failed to connect to bus"), "{r}"),
        other => panic!("expected Unknown, got {other:?}"),
    }
    assert_eq!(st.cron, CronDriverStatus::Missing);
    assert!(st.label().starts_with("unknown"), "{}", st.label());
    let f = build_scheduler_driver_findings(2, &st, &[], now);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].id, "scheduler-tick-driver-unknown");
    assert!(f[0].summary.contains("systemd:"), "{}", f[0].summary);

    // Unknown cron + missing systemd is unknown too, in label and doctor.
    let st = DriverStatus {
        cron: CronDriverStatus::Unknown("no crontab".to_string()),
        systemd: SystemdDriverStatus::Missing,
    };
    assert!(st.label().starts_with("unknown"), "{}", st.label());
    assert_eq!(
        build_scheduler_driver_findings(1, &st, &[], now)[0].id,
        "scheduler-tick-driver-unknown"
    );
    // A confirmed install still wins over the other driver being unknown.
    let st = DriverStatus {
        cron: CronDriverStatus::Installed,
        systemd: SystemdDriverStatus::Unknown("no bus".to_string()),
    };
    assert_eq!(st.label(), "cron (installed)");
    assert!(build_scheduler_driver_findings(1, &st, &[], now).is_empty());
}

#[test]
fn systemd_state_classifiers_recognise_states_and_refuse_the_rest() {
    let out = |stdout: &str, stderr: &str| CommandOutput {
        success: stdout.trim() == "enabled" || stdout.trim() == "active",
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
    };
    assert_eq!(classify_is_enabled(&out("enabled\n", "")), Ok(true));
    for s in [
        "disabled",
        "masked",
        "static",
        "linked",
        "indirect",
        "not-found",
    ] {
        assert_eq!(classify_is_enabled(&out(s, "")), Ok(false), "{s}");
    }
    assert!(classify_is_enabled(&out("", "Failed to connect to bus")).is_err());
    assert!(classify_is_enabled(&out("", "")).is_err());
    assert!(classify_is_enabled(&out("something-new", "")).is_err());
    assert_eq!(classify_is_active(&out("active\n", "")), Ok(true));
    assert_eq!(classify_is_active(&out("inactive\n", "")), Ok(false));
    assert_eq!(classify_is_active(&out("failed\n", "")), Ok(false));
    assert!(classify_is_active(&out("", "Failed to connect to bus")).is_err());
}

#[test]
fn systemd_status_checks_is_active_and_restart_reports_repaired() {
    let m = marker();
    let mut host = FakeHost::new();
    let (_, timer) = names();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    // Enabled but stopped: not driving.
    host.started.clear();
    let st = driver_status_with(&mut host, REPO, &m);
    assert_eq!(st.systemd, SystemdDriverStatus::Stopped);
    assert!(!st.any_installed());
    let f = build_scheduler_driver_findings(1, &st, &[], chrono::Utc::now());
    assert_eq!(f[0].id, "scheduler-tick-not-installed");
    assert!(f[0].summary.contains("not running"), "{}", f[0].summary);

    // Re-running the install restarts it: a repair, not a no-op.
    host.log.clear();
    let r = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::Repaired);
    assert!(host.started.contains(&timer));
    assert!(host.calls().contains(&format!("is-active {timer}")));

    // A disabled timer re-enabled by the install is a repair too.
    host.enabled.clear();
    let r = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::Repaired);
    // And now, truly up to date.
    let r = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::AlreadyUpToDate);
}

#[test]
fn systemd_install_removes_written_units_when_enable_fails() {
    let mut host = FakeHost::new();
    host.enable_fails = true;
    let cron = build_tick_cron_line(Path::new(REPO), Path::new(EXE)).unwrap();
    host.crontab = Some(format!("{cron}\n"));
    let (service, timer) = names();
    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("removed the unit files"), "{msg}");
    assert!(host.unit(&service).is_none() && host.unit(&timer).is_none());
    assert_eq!(
        host.crontab.as_deref(),
        Some(format!("{cron}\n").as_str()),
        "cron kept"
    );

    // An existing install of ours is not torn down by a failed repair.
    let mut host = FakeHost::new();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    let old = host
        .unit(&timer)
        .unwrap()
        .replace("OnBootSec=2min", "OnBootSec=5min");
    host.put_unit(&timer, &old);
    host.enable_fails = true;
    assert!(switch_driver(&mut host, &inv(), Driver::Systemd).is_err());
    assert!(host.unit(&service).is_some() && host.unit(&timer).is_some());
}

#[test]
fn tick_invocation_refuses_trailing_whitespace_and_backslash() {
    for repo in ["/home/op/repo ", "/home/op/repo\t", "/home/op/repo\\"] {
        let err = tick_invocation(Path::new(repo), Path::new(EXE)).unwrap_err();
        assert!(err.to_string().contains("ends in whitespace"), "{err}");
    }
    for exe in ["/opt/aida ", "/opt/aida\\"] {
        assert!(
            tick_invocation(Path::new(REPO), Path::new(exe)).is_err(),
            "{exe}"
        );
    }
    // Inner spaces and backslashes stay fine.
    assert!(tick_invocation(Path::new("/home/op/my repo"), Path::new("/opt/a\\b/aida")).is_ok());
}

#[test]
fn init_tick_offer_uses_the_driver_gate() {
    assert!(init_tick_offer_allowed(true, true, false));
    assert!(!init_tick_offer_allowed(true, true, true), "agent mode");
    assert!(
        !init_tick_offer_allowed(false, true, false),
        "stdin not a tty"
    );
    assert!(
        !init_tick_offer_allowed(true, false, false),
        "stdout not a tty"
    );
}

#[test]
fn shift_install_driver_failure_says_the_shift_is_enabled() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    let layer = root.path().join("home-shift-local.toml");
    let mut host = FakeHost::new();
    host.enable_fails = true;
    let mut yes = |_: &str| Ok(true);
    let mut human = crate::shift::Operator {
        stdin_tty: true,
        agent_mode: false,
        confirm: &mut yes,
    };
    let err =
        crate::shift::install_command(root.path(), &layer, Driver::Systemd, &mut human, &mut host)
            .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("night shift is now enabled"), "{msg}");
    assert!(msg.contains("scheduler driver is unchanged"), "{msg}");
    assert!(std::fs::read_to_string(&layer)
        .unwrap()
        .contains("enabled = true"));
}

// ---------------------------------------------------------------------------
// Operator gate, CLI surface, and the real host's test guards
// ---------------------------------------------------------------------------

#[test]
fn schedule_driver_install_gate_refuses_agents_and_non_tty() {
    assert!(driver_gate_refusal("aida schedule install-systemd", true, false).is_none());
    for (tty, agent) in [(false, false), (true, true), (false, true)] {
        let msg = driver_gate_refusal("aida schedule install-systemd", tty, agent).unwrap();
        assert!(msg.contains("human at an interactive terminal"), "{msg}");
    }
    let root = tempfile::tempdir().unwrap();
    for (tty, agent) in [(false, false), (true, true)] {
        let mut host = FakeHost::new();
        let mut asked = false;
        let mut confirm = |_: &str| {
            asked = true;
            Ok(true)
        };
        let mut op = crate::shift::Operator {
            stdin_tty: tty,
            agent_mode: agent,
            confirm: &mut confirm,
        };
        let err = install_driver_command(
            root.path(),
            "aida schedule install-systemd",
            Driver::Systemd,
            &mut op,
            &mut host,
        )
        .unwrap_err();
        assert!(err.to_string().contains("interactive terminal"));
        assert!(!asked && host.log.is_empty() && host.unit(&names().1).is_none());
    }
    // A human who says no: nothing installed.
    let mut host = FakeHost::new();
    let mut no = |_: &str| Ok(false);
    let mut op = crate::shift::Operator {
        stdin_tty: true,
        agent_mode: false,
        confirm: &mut no,
    };
    let out = install_driver_command(
        root.path(),
        "aida schedule install-cron",
        Driver::Cron,
        &mut op,
        &mut host,
    )
    .unwrap();
    assert!(out.is_none() && host.crontab.is_none() && host.log.is_empty());
}

#[test]
fn shift_install_is_gated_and_installs_driver_after_yes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    let layer = root.path().join("home-shift-local.toml");

    // Agent: refused, nothing written.
    let mut host = FakeHost::new();
    let mut yes = |_: &str| Ok(true);
    let mut agent = crate::shift::Operator {
        stdin_tty: true,
        agent_mode: true,
        confirm: &mut yes,
    };
    assert!(crate::shift::install_command(
        root.path(),
        &layer,
        Driver::Systemd,
        &mut agent,
        &mut host
    )
    .is_err());
    assert!(!layer.exists() && host.log.is_empty());

    // Human, yes: enabled locally and the timer installed + verified.
    let mut yes = |_: &str| Ok(true);
    let mut human = crate::shift::Operator {
        stdin_tty: true,
        agent_mode: false,
        confirm: &mut yes,
    };
    let report =
        crate::shift::install_command(root.path(), &layer, Driver::Systemd, &mut human, &mut host)
            .unwrap()
            .expect("installed");
    assert_eq!(report.driver, Driver::Systemd);
    assert!(std::fs::read_to_string(&layer)
        .unwrap()
        .contains("enabled = true"));
    let repo = root.path().canonicalize().unwrap().display().to_string();
    assert!(host.enabled.contains(&unit_names(&repo).1));
}

#[test]
fn shift_install_and_schedule_systemd_verbs_parse() {
    let cli = Cli::try_parse_from(["aida", "shift", "install", "--systemd-user"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Shift(ShiftCommand::Install {
            systemd_user: true,
            cron: false
        })
    ));
    let cli = Cli::try_parse_from(["aida", "shift", "install", "--cron"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Shift(ShiftCommand::Install {
            systemd_user: false,
            cron: true
        })
    ));
    assert!(Cli::try_parse_from(["aida", "shift", "install"]).is_err());
    assert!(Cli::try_parse_from(["aida", "shift", "install", "--cron", "--systemd-user"]).is_err());
    let cli = Cli::try_parse_from(["aida", "schedule", "install-systemd"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Schedule(MaintenanceScheduleCommand::InstallSystemd)
    ));
    let cli = Cli::try_parse_from(["aida", "schedule", "uninstall-systemd"]).unwrap();
    assert!(matches!(
        cli.command,
        Command::Schedule(MaintenanceScheduleCommand::UninstallSystemd)
    ));
}

#[test]
fn schedule_driver_real_host_refuses_side_effects_under_test() {
    let mut real = RealDriverHost;
    assert!(real.systemctl_user(&["is-enabled", "x.timer"]).is_err());
    assert!(real.unit_dir().is_err());
    assert!(real.write_crontab("").is_err());
    assert_eq!(real.linger_enabled(), None);
}

#[cfg(unix)]
#[test]
fn schedule_driver_unit_dir_prefers_absolute_xdg_config_home() {
    assert_eq!(
        user_unit_dir_from(Some(PathBuf::from("/x/cfg")), Some(PathBuf::from("/h"))),
        Some(PathBuf::from("/x/cfg/systemd/user"))
    );
    assert_eq!(
        user_unit_dir_from(Some(PathBuf::from("rel")), Some(PathBuf::from("/h"))),
        Some(PathBuf::from("/h/.config/systemd/user"))
    );
    assert_eq!(user_unit_dir_from(None, None), None);
}

#[test]
fn schedule_driver_build_output_helper() {
    let checkout = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(checkout.path().join(".git")).unwrap();
    let exe = checkout.path().join("target").join("debug").join("aida");
    assert!(exe_is_build_output(&exe));
    assert!(!exe_is_build_output(Path::new(EXE)));
}

// ---------------------------------------------------------------------------
// BUG-1619: partial-install cleanup and accurate driver messages
// ---------------------------------------------------------------------------

fn cron_body() -> String {
    format!(
        "{}\n",
        build_tick_cron_line(Path::new(REPO), Path::new(EXE)).unwrap()
    )
}

fn failure_files(err: &anyhow::Error) -> UnitFilesAfterFailure {
    err.downcast_ref::<SystemdInstallFailure>()
        .expect("a systemd install failure carries SystemdInstallFailure")
        .files
        .clone()
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_fresh_install_start_failure_disables_then_removes_units() {
    let mut host = FakeHost::new();
    host.start_fails = true;
    host.crontab = Some(cron_body());
    let (service, timer) = names();

    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("removed the unit files"), "{msg}");
    assert_eq!(
        failure_files(&err),
        UnitFilesAfterFailure::CleanedUp {
            timer_enabled: true,
            removed: vec![service.clone(), timer.clone()],
            left: vec![],
            disable_error: None,
        }
    );
    // Disable first (while the timer file still exists), then remove.
    let calls = host.calls();
    let disable = calls
        .iter()
        .position(|c| c == &format!("disable --now {timer}"))
        .expect("the enabled timer is disabled");
    assert!(calls[..disable].contains(&format!("enable {timer}")));
    assert_eq!(calls.last().map(String::as_str), Some("daemon-reload"));
    assert_eq!(host.unit_present_at_disable, vec![true]);
    assert!(!host.enabled.contains(&timer), "no enabled timer left");
    assert!(host.unit(&service).is_none() && host.unit(&timer).is_none());
    assert!(
        host.log.iter().flatten().all(|a| !a.ends_with(".service")),
        "the service is never stopped or disabled"
    );
    assert_eq!(host.crontab, Some(cron_body()), "never zero drivers");
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_fresh_install_post_check_failure_disables_and_removes_units() {
    let mut host = FakeHost::new();
    host.enable_does_not_stick = true;
    host.crontab = Some(cron_body());
    let (service, timer) = names();

    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("is-enabled"), "{msg}");
    assert!(matches!(
        failure_files(&err),
        UnitFilesAfterFailure::CleanedUp { ref left, .. } if left.is_empty()
    ));
    assert!(host.calls().contains(&format!("disable --now {timer}")));
    assert!(
        !host.started.contains(&timer),
        "the started timer is stopped"
    );
    assert!(host.unit(&service).is_none() && host.unit(&timer).is_none());
    assert_eq!(host.crontab, Some(cron_body()), "cron kept");
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_second_unit_write_failure_removes_the_first() {
    let mut host = FakeHost::new();
    host.crontab = Some(cron_body());
    let (service, timer) = names();
    // A directory where the timer's temporary file goes makes that write
    // fail after the service file has been written.
    std::fs::create_dir_all(host.unit_dir.join(format!("{timer}.aida-tmp"))).unwrap();

    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    assert_eq!(
        failure_files(&err),
        UnitFilesAfterFailure::CleanedUp {
            timer_enabled: false,
            removed: vec![service.clone()],
            left: vec![],
            disable_error: None,
        }
    );
    assert!(host.unit(&service).is_none(), "first file removed");
    assert!(
        !host.calls().iter().any(|c| c.starts_with("disable")),
        "nothing was enabled, so nothing is disabled: {:?}",
        host.calls()
    );
    assert_eq!(host.crontab, Some(cron_body()), "cron kept");
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_cleanup_disable_failure_still_removes_marked_units_and_says_so() {
    let mut host = FakeHost::new();
    host.start_fails = true;
    host.disable_fails = true;
    let (service, timer) = names();
    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    match failure_files(&err) {
        UnitFilesAfterFailure::CleanedUp {
            removed,
            disable_error: Some(d),
            ..
        } => {
            assert_eq!(removed, vec![service.clone(), timer.clone()]);
            assert!(d.contains("Failed to disable"), "{d}");
        }
        other => panic!("expected a cleanup with a disable error, got {other:?}"),
    }
    assert!(format!("{err:#}").contains("disabling it failed"));
    assert!(crate::shift::driver_state_after_failure(&err).contains("could not be fully"));
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_cleanup_never_removes_a_file_without_our_marker() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.timer"), format!("# {}\n", marker())).unwrap();
    std::fs::write(dir.path().join("b.service"), "[Service]\n").unwrap();
    let removed = remove_marked_files(dir.path(), &["a.timer", "b.service"], &marker());
    assert_eq!(removed, vec!["a.timer".to_string()]);
    assert!(dir.path().join("b.service").exists());
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_failed_repair_leaves_rewritten_files_and_never_disables() {
    let mut host = FakeHost::new();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    let (service, timer) = names();
    let old = host
        .unit(&timer)
        .unwrap()
        .replace("OnBootSec=2min", "OnBootSec=5min");
    host.put_unit(&timer, &old);
    host.start_fails = true;
    host.log.clear();

    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    assert_eq!(
        failure_files(&err),
        UnitFilesAfterFailure::Rewritten(vec![timer.clone()])
    );
    assert!(format!("{err:#}").contains("left in place"));
    assert!(host.unit(&service).is_some() && host.unit(&timer).is_some());
    assert!(host.unit(&timer).unwrap().contains("OnBootSec=2min"));
    assert!(!host.calls().iter().any(|c| c.starts_with("disable")));

    // A failure with the files already current changes no file.
    let mut host = FakeHost::new();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    host.started.clear();
    host.start_fails = true;
    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    assert_eq!(failure_files(&err), UnitFilesAfterFailure::Unchanged);
    assert!(host.unit(&timer).is_some());
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_repaired_message_names_the_cause() {
    let mut host = FakeHost::new();
    let (_, timer) = names();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();

    // Rewritten files.
    let old = host
        .unit(&timer)
        .unwrap()
        .replace("OnBootSec=2min", "OnBootSec=5min");
    host.put_unit(&timer, &old);
    let r = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::Repaired);
    assert_eq!(r.repair.rewrote, vec![timer.clone()]);
    assert!(!r.repair.re_enabled && !r.repair.restarted);
    let text = r.repair.describe();
    assert!(
        text.contains("rewrote") && text.contains("older invocation"),
        "{text}"
    );

    // Re-enabled (unchanged files): no "older invocation" claim.
    host.enabled.clear();
    host.started.clear();
    let r = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::Repaired);
    assert!(r.repair.rewrote.is_empty() && r.repair.re_enabled);
    let text = r.repair.describe();
    assert!(text.contains("re-enabled"), "{text}");
    assert!(!text.contains("older invocation"), "{text}");

    // Restarted (enabled but stopped).
    host.started.clear();
    let r = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::Repaired);
    assert!(r.repair.restarted && !r.repair.re_enabled);
    let text = r.repair.describe();
    assert!(
        text.contains("started the timer") && text.contains("not running"),
        "{text}"
    );
    assert!(!text.contains("older invocation"), "{text}");

    // Up to date: no cause.
    let r = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::AlreadyUpToDate);
    assert_eq!(r.repair, RepairCauses::default());

    // Cron repair: the entry was rewritten.
    let mut host = FakeHost::new();
    host.crontab = Some(format!(
        "*/15 * * * * cd {REPO} && aida schedule tick --format json\n"
    ));
    let r = switch_driver(&mut host, &inv(), Driver::Cron).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::Repaired);
    assert!(r.repair.describe().contains("rewrote the crontab entry"));
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_shift_install_wording_after_a_failed_repair_or_cleanup() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    let layer = root.path().join("home-shift-local.toml");
    let repo = root.path().canonicalize().unwrap().display().to_string();
    let (_, timer) = unit_names(&repo);
    let run = |host: &mut FakeHost| {
        let mut yes = |_: &str| Ok(true);
        let mut human = crate::shift::Operator {
            stdin_tty: true,
            agent_mode: false,
            confirm: &mut yes,
        };
        format!(
            "{:#}",
            crate::shift::install_command(root.path(), &layer, Driver::Systemd, &mut human, host)
                .unwrap_err()
        )
    };

    // Fresh install cleaned up after a start failure: unchanged is true.
    let mut host = FakeHost::new();
    host.start_fails = true;
    let msg = run(&mut host);
    assert!(msg.contains("disabled and removed"), "{msg}");
    assert!(msg.contains("scheduler driver is unchanged"), "{msg}");

    // A repair that rewrote the files and then failed: never "unchanged".
    let mut host = FakeHost::new();
    host.start_fails = false;
    let mut yes = |_: &str| Ok(true);
    let mut human = crate::shift::Operator {
        stdin_tty: true,
        agent_mode: false,
        confirm: &mut yes,
    };
    crate::shift::install_command(root.path(), &layer, Driver::Systemd, &mut human, &mut host)
        .unwrap();
    let old = host
        .unit(&timer)
        .unwrap()
        .replace("OnBootSec=2min", "OnBootSec=5min");
    host.put_unit(&timer, &old);
    host.start_fails = true;
    let msg = run(&mut host);
    assert!(msg.contains("rewritten before the failure"), "{msg}");
    assert!(!msg.contains("unchanged"), "{msg}");

    // An error that is not a systemd install failure keeps the hedge.
    let other = anyhow::anyhow!("something else");
    assert!(crate::shift::driver_state_after_failure(&other).starts_with("Unless"));
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_shift_enable_hint_reports_unknown_not_needed() {
    let m = marker();
    let mut host = FakeHost::new();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    host.no_bus = true;
    let st = driver_status_with(&mut host, REPO, &m);
    let hint = crate::shift::driver_hint(&st).expect("an unknown driver is reported");
    assert!(hint.contains(&st.label()), "{hint}");
    assert!(hint.contains("unknown"), "{hint}");
    assert!(!hint.contains("needed"), "{hint}");

    // Nothing installed and nothing unknown: still "needed".
    let none = driver_status_with(&mut FakeHost::new(), REPO, &m);
    assert!(crate::shift::driver_hint(&none)
        .unwrap()
        .contains("needed: a scheduler driver"));

    // Installed: no hint.
    let mut host = FakeHost::new();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    let st = driver_status_with(&mut host, REPO, &m);
    assert_eq!(crate::shift::driver_hint(&st), None);
}

// ---------------------------------------------------------------------------
// BUG-1619 rework: fresh-install guard, wording, hint, temp file
// ---------------------------------------------------------------------------

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_fresh_install_refuses_when_systemd_knows_the_timer_elsewhere() {
    for state in ["enabled", "disabled", "static", "masked", "linked"] {
        let mut host = FakeHost::new();
        host.elsewhere = Some(state);
        host.crontab = Some(cron_body());
        let (service, timer) = names();

        let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("already knows"), "{state}: {msg}");
        assert!(msg.contains("different unit directory"), "{state}: {msg}");
        assert_eq!(failure_files(&err), UnitFilesAfterFailure::Unchanged);
        // Nothing written, nothing enabled or disabled: only the probe ran.
        assert_eq!(host.calls(), vec![format!("is-enabled {timer}")], "{state}");
        assert!(host.unit(&service).is_none() && host.unit(&timer).is_none());
        assert!(!host.unit_dir.exists(), "{state}: no directory created");
        assert_eq!(host.crontab, Some(cron_body()), "{state}: cron kept");
        assert!(crate::shift::driver_state_after_failure(&err).contains("unchanged"));
    }
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_fresh_install_refuses_when_systemd_state_is_unknown() {
    let mut host = FakeHost::new();
    host.no_bus = true;
    host.crontab = Some(cron_body());
    let (service, timer) = names();

    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("could not tell whether systemd already knows"),
        "{msg}"
    );
    assert!(msg.contains("Failed to connect to bus"), "{msg}");
    assert_eq!(failure_files(&err), UnitFilesAfterFailure::Unchanged);
    assert_eq!(host.calls(), vec![format!("is-enabled {timer}")]);
    assert!(host.unit(&service).is_none() && host.unit(&timer).is_none());
    assert_eq!(host.crontab, Some(cron_body()), "cron kept");
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_fresh_install_proceeds_when_systemd_reports_not_found() {
    for legacy in [false, true] {
        let mut host = FakeHost::new();
        host.legacy_not_found = legacy;
        host.crontab = Some(cron_body());
        let (service, timer) = names();

        let r = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
        assert_eq!(
            r.outcome,
            DriverInstallOutcome::Installed,
            "legacy={legacy}"
        );
        let calls = host.calls();
        assert_eq!(calls[0], format!("is-enabled {timer}"), "probe runs first");
        assert!(calls.contains(&format!("enable {timer}")), "{calls:?}");
        assert!(host.unit(&service).is_some() && host.unit(&timer).is_some());
        // New driver verified, then cron removed.
        assert!(!host.crontab.clone().unwrap_or_default().contains(REPO));
    }
    // A repair (our files present) never runs the fresh probe's refusal.
    let mut host = FakeHost::new();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    host.elsewhere = Some("enabled");
    host.enabled.clear();
    let r = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();
    assert_eq!(r.outcome, DriverInstallOutcome::Repaired);
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_fresh_probe_classifier() {
    let out = |success: bool, stdout: &str, stderr: &str| CommandOutput {
        success,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
    };
    let probe = |o: &CommandOutput| classify_fresh_probe(o, "x.timer");
    assert_eq!(probe(&out(false, "not-found\n", "")), FreshProbe::NotFound);
    assert_eq!(
        probe(&out(
            false,
            "",
            "Failed to get unit file state for x.timer: No such file or directory\n"
        )),
        FreshProbe::NotFound
    );
    assert_eq!(
        probe(&out(false, "disabled\n", "")),
        FreshProbe::Known("disabled".to_string())
    );
    assert_eq!(
        probe(&out(true, "enabled\n", "")),
        FreshProbe::Known("enabled".to_string())
    );
    assert!(matches!(
        probe(&out(
            false,
            "",
            "Failed to connect to bus: No medium found\n"
        )),
        FreshProbe::Unknown(_)
    ));
    assert!(matches!(
        probe(&out(true, "bogus\n", "")),
        FreshProbe::Unknown(_)
    ));
}

// The real no-user-bus error (systemd 255: exit 1, empty stdout) also ends
// in "No such file or directory"; it must be Unknown, never NotFound.
// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_fresh_probe_real_bus_enoent_is_unknown() {
    let out = |success: bool, stdout: &str, stderr: &str| CommandOutput {
        success,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
    };
    let real_bus = "Failed to connect to bus: No such file or directory\n";
    match classify_fresh_probe(&out(false, "", real_bus), "x.timer") {
        FreshProbe::Unknown(why) => {
            assert!(why.contains("Failed to connect to bus"), "{why}")
        }
        other => panic!("bus ENOENT classified as {other:?}"),
    }
    // Bus failure wins even if stdout claimed not-found.
    assert!(matches!(
        classify_fresh_probe(&out(false, "not-found\n", real_bus), "x.timer"),
        FreshProbe::Unknown(_)
    ));
    // Other bus/connection failures are Unknown too.
    for stderr in [
        "Failed to connect to user scope bus via local transport: No such file or directory\n",
        "Failed to get D-Bus connection: No such file or directory\n",
        "Failed to connect to bus: Connection refused\n",
    ] {
        assert!(
            matches!(
                classify_fresh_probe(&out(false, "", stderr), "x.timer"),
                FreshProbe::Unknown(_)
            ),
            "{stderr}"
        );
    }
    // The legacy not-found path needs the is-enabled prefix for this exact
    // timer; any other ENOENT text is ambiguous and fails closed.
    for stderr in [
        "No such file or directory\n",
        "Failed to get unit file state for other.timer: No such file or directory\n",
        "Failed to get unit file state for x.timer: Permission denied\n",
        "Something else: No such file or directory\n",
    ] {
        assert!(
            matches!(
                classify_fresh_probe(&out(false, "", stderr), "x.timer"),
                FreshProbe::Unknown(_)
            ),
            "{stderr}"
        );
    }
}

// Both refusal messages read as prose: no runs of spaces left by a lost
// line continuation. trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_fresh_install_refusals_have_no_double_spaces() {
    let mut host = FakeHost::new();
    host.elsewhere = Some("enabled");
    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    let known = format!("{err:#}");
    assert!(known.contains("already knows"), "{known}");
    assert!(!known.contains("  "), "double space in: {known}");

    let mut host = FakeHost::new();
    host.no_bus = true;
    let err = switch_driver(&mut host, &inv(), Driver::Systemd).unwrap_err();
    let unknown = format!("{err:#}");
    assert!(unknown.contains("could not tell"), "{unknown}");
    assert!(!unknown.contains("  "), "double space in: {unknown}");
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_second_write_failure_wording_says_nothing_was_enabled() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    let layer = root.path().join("home-shift-local.toml");
    let repo = root.path().canonicalize().unwrap().display().to_string();
    let (_, timer) = unit_names(&repo);
    let mut host = FakeHost::new();
    std::fs::create_dir_all(host.unit_dir.join(format!("{timer}.aida-tmp"))).unwrap();
    let mut yes = |_: &str| Ok(true);
    let mut human = crate::shift::Operator {
        stdin_tty: true,
        agent_mode: false,
        confirm: &mut yes,
    };
    let err =
        crate::shift::install_command(root.path(), &layer, Driver::Systemd, &mut human, &mut host)
            .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("No timer was enabled"), "{msg}");
    assert!(
        msg.contains("unit file(s) this install wrote were removed"),
        "{msg}"
    );
    assert!(!msg.contains("disabled and removed"), "{msg}");
    assert!(!host.calls().iter().any(|c| c.starts_with("disable")));
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_shift_enable_hint_matches_label_for_disabled_and_stopped() {
    let m = marker();
    let mut host = FakeHost::new();
    switch_driver(&mut host, &inv(), Driver::Systemd).unwrap();

    // Disabled.
    host.enabled.clear();
    host.started.clear();
    let st = driver_status_with(&mut host, REPO, &m);
    assert_eq!(st.systemd, SystemdDriverStatus::Disabled);
    let hint = crate::shift::driver_hint(&st).expect("a disabled timer is reported");
    assert!(hint.contains(&st.label()), "{hint}");
    assert!(hint.contains("installed but disabled"), "{hint}");
    assert!(!hint.contains("needed"), "{hint}");

    // Enabled but stopped.
    let (_, timer) = names();
    host.enabled.insert(timer);
    let st = driver_status_with(&mut host, REPO, &m);
    assert_eq!(st.systemd, SystemdDriverStatus::Stopped);
    let hint = crate::shift::driver_hint(&st).expect("a stopped timer is reported");
    assert!(hint.contains(&st.label()), "{hint}");
    assert!(hint.contains("not running"), "{hint}");
    assert!(!hint.contains("needed"), "{hint}");
}

// trace:BUG-1619 | ai:claude
#[test]
fn bug_1619_failed_rename_removes_the_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    // A non-empty directory at the target makes the rename fail.
    let target = dir.path().join("x.timer");
    std::fs::create_dir_all(target.join("inner")).unwrap();
    assert!(write_atomic(&target, "body").is_err());
    assert!(!atomic_tmp_path(&target).exists(), "temp file removed");
}

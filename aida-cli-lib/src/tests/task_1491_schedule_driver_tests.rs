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
    render_tick_cron_line, tick_invocation, CronDriverStatus, DriverInstallOutcome,
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
        Ok(match args {
            ["enable", unit] => {
                if !self.enable_does_not_stick {
                    self.enabled.insert(unit.to_string());
                }
                ok("")
            }
            ["disable", "--now", unit] => {
                self.enabled.remove(*unit);
                ok("")
            }
            ["is-enabled", unit] => {
                if self.enabled.contains(*unit) {
                    ok("enabled\n")
                } else {
                    CommandOutput {
                        success: false,
                        stdout: "disabled\n".to_string(),
                        stderr: String::new(),
                    }
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
    assert!(err.to_string().contains("is-enabled"), "{err}");
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
    // Flagged even with no registered jobs.
    assert_eq!(build_scheduler_driver_findings(0, &both, &[], now).len(), 1);

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

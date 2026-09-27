//! TASK-1517: per-wave resource limits on the `systemd-run --user` drain
//! wave unit, and the doctor warning when the user manager cannot enforce
//! them.
//!
//! Every test is pure or goes through the fake [`DriverHost`] of the
//! TASK-1510 suite: nothing runs `systemd-run`, `systemctl` or `aida`,
//! nothing reads the live store or the real `~/.aida`, and no wave is ever
//! launched. The limits only CONSTRAIN a wave — no test here enables,
//! schedules or starts anything.
// trace:TASK-1517 | ai:claude

use super::task_1510_wave_unit_tests::{ctx, probes, TickExec, WaveHost};
use super::*;
use crate::schedule_driver::{
    build_wave_limit_findings, build_wave_limits, classify_wave_limit_delegation,
    user_manager_cgroup_path, BoundedRun, CommandOutput, DriverHost, SystemdDriverStatus,
    WaveLimitDelegation, WaveLimitInput, WaveLimits, WAVE_LIMIT_CONTROLLERS,
};
use chrono::TimeZone;

const REPO: &str = "/repo/aida";
const EXE: &str = "/usr/local/bin/aida";
const PATH_ENV: &str = "/usr/local/bin:/usr/bin:/bin";
const LOG: &str = "/repo/aida/.aida/shift-wave-20260924-221000.log";
const TICK_PID: u32 = 777;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 24, 22, 10, 0).unwrap()
}

fn cfg_with(committed: &str) -> ShiftConfig {
    let local: toml::Value =
        toml::from_str(&format!("[repo.\"{REPO}\"]\nenabled = true\n")).unwrap();
    let committed: toml::Value = toml::from_str(committed).unwrap();
    build_config(Some(&committed), Some(&local), REPO, "L")
}

fn cfg_on() -> ShiftConfig {
    let local: toml::Value =
        toml::from_str(&format!("[repo.\"{REPO}\"]\nenabled = true\n")).unwrap();
    build_config(None, Some(&local), REPO, "L")
}

fn limits_of(toml_body: &str) -> (WaveLimits, Option<String>) {
    let v: toml::Value = toml::from_str(toml_body).unwrap();
    let t = v.get("shift").unwrap();
    build_wave_limits(&WaveLimitInput {
        memory_high: t.get("wave_memory_high"),
        memory_max: t.get("wave_memory_max"),
        cpu_weight: t.get("wave_cpu_weight"),
        io_weight: t.get("wave_io_weight"),
        tasks_max: t.get("wave_tasks_max"),
    })
}

/// A host that answers the wave launch and PANICS on anything a launch must
/// never do — including reading cgroup delegation: the limits are applied
/// unconditionally, never gated on a probe (fail open).
struct LimitHost {
    runs: Vec<Vec<String>>,
}

impl DriverHost for LimitHost {
    fn read_crontab(&mut self) -> Result<Option<String>> {
        panic!("a wave launch never reads the crontab")
    }
    fn write_crontab(&mut self, _: &str) -> Result<()> {
        panic!("a wave launch never writes the crontab")
    }
    fn systemctl_user(&mut self, args: &[&str]) -> Result<CommandOutput> {
        panic!("a wave launch never runs an unbounded systemctl: {args:?}")
    }
    fn unit_dir(&self) -> Result<PathBuf> {
        panic!("a wave launch never touches the unit directory")
    }
    fn linger_enabled(&mut self) -> Option<bool> {
        panic!("a wave launch never reads linger")
    }
    fn systemd_supported(&self) -> bool {
        true
    }
    fn systemd_run_user(&mut self, args: &[String], _: StdDuration) -> BoundedRun {
        self.runs.push(args.to_vec());
        BoundedRun::Exited(CommandOutput {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
    fn systemctl_user_bounded(&mut self, _: &[&str], _: StdDuration) -> BoundedRun {
        BoundedRun::Exited(CommandOutput {
            success: true,
            stdout: "LoadState=loaded\nActiveState=active\nMainPID=5151\n".to_string(),
            stderr: String::new(),
        })
    }
    fn self_cgroup(&self) -> Option<String> {
        Some(format!(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/{}.service\n",
            crate::schedule_driver::unit_stem(REPO)
        ))
    }
    fn cgroup_controllers(&self) -> Option<String> {
        panic!("a wave launch never reads cgroup delegation: the limits are not probe-gated")
    }
}

/// The `systemd-run` argv one launch under `cfg` produces, plus how many
/// times the detached fallback was taken.
fn launch_argv(cfg: &ShiftConfig) -> (Vec<String>, usize) {
    let mut host = LimitHost { runs: Vec::new() };
    let argv = build_wave_argv(cfg, "shift-20260924-2210", 50_000_000);
    let mut detached = 0usize;
    let mut launcher = || -> Result<(u32, Option<String>)> {
        detached += 1;
        Ok((9090, None))
    };
    let w = WaveLaunch {
        setting: cfg.wave_unit,
        invoker: Some("systemd"),
        repo: REPO,
        exe: EXE,
        path_env: PATH_ENV,
        argv: &argv,
        log: LOG,
        now: now(),
        tick_pid: TICK_PID,
        runtime_max_secs: cfg.wave_runtime_max_secs(),
        limits: cfg.wave_limits_for_launch(),
    };
    let identity = |pid: u32| Some(format!("start-{pid}"));
    let result = launch_wave_isolated(&mut host, &w, &mut launcher, &identity);
    assert!(result.is_ok(), "{:?}", result.err());
    (host.runs.pop().expect("systemd-run was called"), detached)
}

fn properties(args: &[String]) -> Vec<String> {
    let end = args.iter().position(|a| a == "--").expect("a `--`");
    args[..end]
        .windows(2)
        .filter(|w| w[0] == "-p")
        .map(|w| w[1].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// The limits reach the unit
// ---------------------------------------------------------------------------

#[test]
fn default_limits_are_the_advisor_shares_and_reach_the_unit() {
    // 40%/50% of total memory: on the 70 GB machine of the 2026-09-25
    // incident that is a 28 GiB reclaim throttle and a 35 GiB hard wall, so
    // the wave is throttled below the 30 GB the runaway reached and any kill
    // lands inside the wave's own cgroup.
    let cfg = cfg_on();
    assert_eq!(cfg.wave_limits, WaveLimits::default());
    assert_eq!(cfg.wave_limits.memory_high, "40%");
    assert_eq!(cfg.wave_limits.memory_max, "50%");
    assert_eq!(cfg.wave_limits.cpu_weight, 50);
    assert_eq!(cfg.wave_limits.io_weight, 50);
    assert_eq!(cfg.wave_limits.tasks_max, 2048);
    assert!(cfg.wave_limits_error.is_none());
    let (args, detached) = launch_argv(&cfg);
    assert_eq!(detached, 0);
    let props = properties(&args);
    for want in [
        "MemoryHigh=40%",
        "MemoryMax=50%",
        "CPUWeight=50",
        "IOWeight=50",
        "TasksMax=2048",
    ] {
        assert_eq!(
            props.iter().filter(|p| p.as_str() == want).count(),
            1,
            "{want} exactly once in {props:?}"
        );
    }
}

#[test]
fn configured_limits_reach_the_unit() {
    let cfg = cfg_with(
        "[shift]\nwave_memory_high = \"12G\"\nwave_memory_max = \"16G\"\n\
         wave_cpu_weight = 20\nwave_io_weight = 10\nwave_tasks_max = 512\n",
    );
    assert!(
        cfg.wave_limits_error.is_none(),
        "{:?}",
        cfg.wave_limits_error
    );
    let props = properties(&launch_argv(&cfg).0);
    for want in [
        format!("MemoryHigh={}", 12u64 * 1024 * 1024 * 1024),
        format!("MemoryMax={}", 16u64 * 1024 * 1024 * 1024),
        "CPUWeight=20".to_string(),
        "IOWeight=10".to_string(),
        "TasksMax=512".to_string(),
    ] {
        assert!(props.contains(&want), "{want} in {props:?}");
    }
}

#[test]
fn the_local_layer_overrides_the_committed_limits() {
    let committed: toml::Value = toml::from_str(
        "[shift]\nwave_memory_high = \"40%\"\nwave_memory_max = \"60%\"\nwave_cpu_weight = 60\nwave_io_weight = 60\nwave_tasks_max = 1024\n",
    )
    .unwrap();
    let local: toml::Value = toml::from_str(&format!(
        "[repo.\"{REPO}\"]\nenabled = true\nwave_memory_high = \"20%\"\nwave_memory_max = \"30%\"\nwave_cpu_weight = 20\nwave_io_weight = 10\nwave_tasks_max = 512\n"
    ))
    .unwrap();
    let cfg = build_config(Some(&committed), Some(&local), REPO, "L");
    assert_eq!(cfg.wave_limits.memory_high, "20%");
    assert_eq!(cfg.wave_limits.memory_max, "30%");
    assert_eq!(cfg.wave_limits.cpu_weight, 20);
    assert_eq!(cfg.wave_limits.io_weight, 10);
    assert_eq!(cfg.wave_limits.tasks_max, 512);
}

#[test]
fn limits_are_properties_only_and_never_reach_the_wave_command() {
    // The limits must not change what the wave runs, only what it may use.
    let cfg = cfg_on();
    let (args, _) = launch_argv(&cfg);
    let end = args.iter().position(|a| a == "--").unwrap();
    let command = &args[end + 1..];
    for banned in [
        "Memory", "Weight", "TasksMax", "--scope", "Restart", "--on-", "Delegate",
    ] {
        assert!(
            !command.iter().any(|a| a.contains(banned)),
            "{banned} in the wave command {command:?}"
        );
    }
    assert_eq!(command[0], EXE);
    assert_eq!(
        &command[1..],
        build_wave_argv(&cfg, "shift-20260924-2210", 50_000_000).as_slice()
    );
}

// ---------------------------------------------------------------------------
// Fail closed on configuration
// ---------------------------------------------------------------------------

#[test]
fn unusable_limits_keep_the_defaults_and_report_one_error() {
    for body in [
        "[shift]\nwave_memory_high = \"lots\"\n",
        "[shift]\nwave_memory_high = \"0%\"\n",
        "[shift]\nwave_memory_max = \"150%\"\n",
        "[shift]\nwave_memory_max = \"infinity\"\n",
        "[shift]\nwave_memory_max = 0\n",
        "[shift]\nwave_memory_max = true\n",
        "[shift]\nwave_memory_high = \"60%\"\nwave_memory_max = \"50%\"\n",
        "[shift]\nwave_memory_high = \"16G\"\nwave_memory_max = \"8G\"\n",
        "[shift]\nwave_cpu_weight = 0\n",
        "[shift]\nwave_cpu_weight = 10001\n",
        "[shift]\nwave_io_weight = -5\n",
        "[shift]\nwave_io_weight = \"low\"\n",
        "[shift]\nwave_tasks_max = 0\n",
        "[shift]\nwave_tasks_max = 1.5\n",
    ] {
        let (limits, error) = limits_of(body);
        assert!(error.is_some(), "{body} should not be usable");
        assert_eq!(limits, WaveLimits::default(), "{body}");
    }
}

#[test]
fn a_percentage_and_a_size_are_not_compared() {
    // Ordering a share of the machine against an absolute size needs the
    // machine; leaving it to systemd is not an error.
    let (limits, error) =
        limits_of("[shift]\nwave_memory_high = \"90%\"\nwave_memory_max = \"8G\"\n");
    assert!(error.is_none(), "{error:?}");
    assert_eq!(limits.memory_high, "90%");
    assert_eq!(limits.memory_max, (8u64 * 1024 * 1024 * 1024).to_string());
}

#[test]
fn memory_sizes_accept_systemd_suffixes() {
    for (raw, bytes) in [
        ("\"1024\"", 1024u64),
        ("1024", 1024),
        ("\"512K\"", 512 * 1024),
        ("\"512m\"", 512 * 1024 * 1024),
        ("\"8G\"", 8 * 1024 * 1024 * 1024),
        ("\"1T\"", 1024u64.pow(4)),
        ("\"4096B\"", 4096),
    ] {
        let (limits, error) = limits_of(&format!(
            "[shift]\nwave_memory_high = {raw}\nwave_memory_max = \"1T\"\n"
        ));
        assert!(error.is_none(), "{raw}: {error:?}");
        assert_eq!(limits.memory_high, bytes.to_string(), "{raw}");
    }
}

#[test]
fn the_wave_limits_guard_refuses_the_launch_on_both_paths() {
    for committed in [
        "[shift]\nwave_memory_max = \"nope\"\n",
        "[shift]\nwave_tasks_max = 0\n",
        // Path-independent: also with the unit switch off.
        "[shift]\nwave_memory_max = \"nope\"\nwave_unit = \"off\"\n",
    ] {
        let cfg = cfg_with(committed);
        assert!(cfg.wave_limits_error.is_some(), "{committed}");
        assert!(cfg.wave_limits_for_launch().is_none(), "{committed}");
        let mut exec = TickExec::new(WaveHost::systemd());
        let mut state = ShiftState::default();
        let r = tick_core(&cfg, &probes(), &mut state, &ctx(), &mut exec).unwrap();
        let g = r.guards.iter().find(|g| g.name == "wave-limits").unwrap();
        assert!(!g.pass, "{committed}: {g:?}");
        assert!(
            r.launched.is_none() && exec.detached == 0 && exec.host.runs.is_empty(),
            "{committed}: nothing may be launched"
        );
    }
}

#[test]
fn the_wave_limits_guard_passes_and_shows_the_limits() {
    let r = tick_core(
        &cfg_on(),
        &probes(),
        &mut ShiftState::default(),
        &ctx(),
        &mut TickExec::new(WaveHost::systemd()),
    )
    .unwrap();
    let g = r
        .guards
        .iter()
        .find(|g| g.name == "wave-limits")
        .expect("a wave-limits verdict");
    assert!(g.pass);
    assert_eq!(g.detail, WaveLimits::default().describe());
    assert!(!g.detail.contains("TASK-"), "no spec id in operator prose");
}

#[test]
fn the_unit_launch_fails_closed_without_usable_limits() {
    // A backstop below the guard: an unlimited wave is exactly what this
    // removes, so the unit path refuses rather than starting one, and it
    // never silently becomes a detached (unlimited) wave instead.
    let mut host = LimitHost { runs: Vec::new() };
    let argv = build_wave_argv(&cfg_on(), "shift-20260924-2210", 50_000_000);
    let mut detached = 0usize;
    let mut launcher = || -> Result<(u32, Option<String>)> {
        detached += 1;
        Ok((9090, None))
    };
    let w = WaveLaunch {
        setting: WaveUnitSetting::Auto,
        invoker: Some("systemd"),
        repo: REPO,
        exe: EXE,
        path_env: PATH_ENV,
        argv: &argv,
        log: LOG,
        now: now(),
        tick_pid: TICK_PID,
        runtime_max_secs: cfg_on().wave_runtime_max_secs(),
        limits: None,
    };
    let identity = |pid: u32| Some(format!("start-{pid}"));
    let err = launch_wave_isolated(&mut host, &w, &mut launcher, &identity)
        .expect_err("no usable limits must not launch a wave");
    assert!(format!("{err:#}").contains("resource limits"), "{err:#}");
    assert_eq!(detached, 0);
    assert!(host.runs.is_empty());
}

// ---------------------------------------------------------------------------
// Delegation: fail open, doctor warns
// ---------------------------------------------------------------------------

#[test]
fn delegation_is_classified_from_the_user_managers_controllers() {
    assert_eq!(
        classify_wave_limit_delegation(Some("cpu io memory pids\n")),
        WaveLimitDelegation::Delegated
    );
    // The incident machine: cpu, memory and pids delegated, io not.
    assert_eq!(
        classify_wave_limit_delegation(Some("cpu memory pids\n")),
        WaveLimitDelegation::Missing(vec!["io".to_string()])
    );
    assert_eq!(
        classify_wave_limit_delegation(Some("cpu pids\n")),
        WaveLimitDelegation::Missing(vec!["memory".to_string(), "io".to_string()])
    );
    assert!(matches!(
        classify_wave_limit_delegation(None),
        WaveLimitDelegation::Unknown(_)
    ));
    assert!(matches!(
        classify_wave_limit_delegation(Some("   \n")),
        WaveLimitDelegation::Unknown(_)
    ));
    assert!(WAVE_LIMIT_CONTROLLERS.contains(&"memory"));
}

#[test]
fn doctor_warns_only_when_the_memory_ceiling_cannot_be_enforced() {
    assert!(build_wave_limit_findings(&WaveLimitDelegation::Delegated).is_empty());
    // A machine that only lacks `io` still enforces the memory ceiling.
    assert!(
        build_wave_limit_findings(&WaveLimitDelegation::Missing(vec!["io".to_string()])).is_empty()
    );
    let f = build_wave_limit_findings(&WaveLimitDelegation::Missing(vec![
        "memory".to_string(),
        "io".to_string(),
    ]));
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].category, "scheduler-driver");
    assert_eq!(f[0].id, "shift-wave-limits-undelegated");
    assert!(f[0].summary.contains("memory controller"));
    // Warn only: nothing to heal, and the wave still runs.
    assert!(!f[0].safe_heal);
    assert!(f[0].summary.contains("still run"));
    assert!(
        !f[0].summary.contains("TASK-"),
        "no spec id in doctor prose"
    );
    let u = build_wave_limit_findings(&WaveLimitDelegation::Unknown("no cgroup2".to_string()));
    assert_eq!(u.len(), 1);
    assert_eq!(u[0].id, "shift-wave-limits-delegation-unknown");
    assert!(u[0].summary.contains("unknown, not ok"));
    assert!(!u[0].safe_heal);
}

#[test]
fn missing_delegation_never_changes_how_a_wave_is_launched() {
    // Fail open. `LimitHost::cgroup_controllers` panics, so a launch that
    // consulted delegation could not pass this test: the limits are always
    // asked for, and a user manager that cannot apply one simply does not.
    let (args, detached) = launch_argv(&cfg_on());
    assert_eq!(detached, 0);
    assert!(properties(&args)
        .iter()
        .any(|p| p.starts_with("MemoryMax=")));
}

#[test]
fn the_user_manager_cgroup_is_found_in_a_cgroup_body() {
    assert_eq!(
        user_manager_cgroup_path(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/aida-wave-x.service\n"
        )
        .as_deref(),
        Some("/user.slice/user-1000.slice/user@1000.service")
    );
    assert_eq!(
        user_manager_cgroup_path("0::/user.slice/user-0.slice/user@0.service\n").as_deref(),
        Some("/user.slice/user-0.slice/user@0.service")
    );
    // A system service, a container, an empty body: no user manager.
    assert_eq!(
        user_manager_cgroup_path("0::/system.slice/sshd.service\n"),
        None
    );
    assert_eq!(user_manager_cgroup_path(""), None);
    // A v1-style body: the v2 line still answers.
    assert_eq!(
        user_manager_cgroup_path(
            "1:name=systemd:/user.slice/user-1000.slice/session-2.scope\n\
             0::/user.slice/user-1000.slice/user@1000.service/app.slice/x.scope\n"
        )
        .as_deref(),
        Some("/user.slice/user-1000.slice/user@1000.service")
    );
}

#[test]
fn the_delegation_check_only_reads_where_a_wave_unit_is_possible() {
    use crate::maintenance_schedule::wave_limit_delegation_check_needed;
    for status in [
        SystemdDriverStatus::Installed,
        SystemdDriverStatus::Disabled,
        SystemdDriverStatus::Stopped,
    ] {
        assert!(wave_limit_delegation_check_needed(&status), "{status:?}");
    }
    for status in [
        SystemdDriverStatus::Missing,
        SystemdDriverStatus::Unsupported,
        SystemdDriverStatus::Unknown("no bus".to_string()),
    ] {
        assert!(!wave_limit_delegation_check_needed(&status), "{status:?}");
    }
}

// ---------------------------------------------------------------------------
// Scope: this constrains a wave, it does not authorise one
// ---------------------------------------------------------------------------

#[test]
fn the_limits_add_no_way_to_start_a_wave() {
    let driver = include_str!("../schedule_driver.rs");
    let section = driver
        .split("// Per-wave resource limits (TASK-1517)")
        .nth(1)
        .expect("the TASK-1517 section");
    let section = section.split("// Commands").next().unwrap();
    for banned in [
        "OnCalendar",
        "OnActiveSec",
        "OnUnitInactiveSec",
        "Restart=",
        "--scope",
        "Command::new",
        "systemd_run_user",
        "spawn(",
    ] {
        assert!(
            !section.contains(banned),
            "the wave-limit code must not contain {banned:?}"
        );
    }
    // The doctor advice never tells the operator to enable or start anything.
    let action =
        &build_wave_limit_findings(&WaveLimitDelegation::Unknown("x".to_string()))[0].action;
    for banned in [
        "shift enable",
        "shift install",
        "queue work",
        "systemctl start",
    ] {
        assert!(!action.contains(banned), "{banned} in {action:?}");
    }
}

//! TASK-1510: each unattended drain wave in its own `systemd-run --user`
//! transient unit. Every test goes through a fake [`DriverHost`] and a
//! recording detached launcher: no test runs `systemd-run`, `systemctl` or
//! `aida`, and nothing is ever started on the machine.
// trace:TASK-1510 | ai:claude

use super::*;
use crate::schedule_driver::{
    cgroup_in_unit, classify_systemd_run, classify_unit_show, unit_stem, wave_unit_name,
    BoundedRun, CommandOutput, DriverHost, RealDriverHost, RunFailure, UnitProbe,
    WAVE_UNIT_PROBE_TIMEOUT, WAVE_UNIT_RUN_TIMEOUT,
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

fn cfg_on() -> ShiftConfig {
    let local: toml::Value =
        toml::from_str(&format!("[repo.\"{REPO}\"]\nenabled = true\n")).unwrap();
    build_config(None, Some(&local), REPO, "L")
}

fn cfg_with(committed: &str) -> ShiftConfig {
    let local: toml::Value =
        toml::from_str(&format!("[repo.\"{REPO}\"]\nenabled = true\n")).unwrap();
    let committed: toml::Value = toml::from_str(committed).unwrap();
    build_config(Some(&committed), Some(&local), REPO, "L")
}

fn wave_argv() -> Vec<String> {
    build_wave_argv(&cfg_on(), "shift-20260924-2210", 50_000_000)
}

/// `/proc/self/cgroup` of a tick started by this repo's timer.
fn tick_cgroup(repo: &str) -> String {
    format!(
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/{}.service\n",
        unit_stem(repo)
    )
}

fn exited(success: bool, stdout: &str, stderr: &str) -> BoundedRun {
    BoundedRun::Exited(CommandOutput {
        success,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
    })
}

fn show_active(pid: u32) -> BoundedRun {
    exited(
        true,
        &format!("LoadState=loaded\nActiveState=active\nMainPID={pid}\n"),
        "",
    )
}

fn show_not_found() -> BoundedRun {
    exited(
        true,
        "LoadState=not-found\nActiveState=inactive\nMainPID=0\n",
        "",
    )
}

/// A fake host: `systemd-run` and `show` answers are scripted, every call is
/// recorded, and anything else a wave launch must never do panics.
struct WaveHost {
    supported: bool,
    cgroup: Option<String>,
    run: BoundedRun,
    show: std::collections::VecDeque<BoundedRun>,
    runs: Vec<Vec<String>>,
    run_timeouts: Vec<StdDuration>,
    shows: Vec<Vec<String>>,
    show_timeouts: Vec<StdDuration>,
}

impl WaveHost {
    fn systemd() -> Self {
        Self {
            supported: true,
            cgroup: Some(tick_cgroup(REPO)),
            run: exited(true, "", ""),
            show: std::collections::VecDeque::new(),
            runs: Vec::new(),
            run_timeouts: Vec::new(),
            shows: Vec::new(),
            show_timeouts: Vec::new(),
        }
    }

    fn with_show(mut self, answers: Vec<BoundedRun>) -> Self {
        self.show = answers.into();
        self
    }
}

impl DriverHost for WaveHost {
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
        self.supported
    }
    fn systemd_run_user(&mut self, args: &[String], timeout: StdDuration) -> BoundedRun {
        self.runs.push(args.to_vec());
        self.run_timeouts.push(timeout);
        self.run.clone()
    }
    fn systemctl_user_bounded(&mut self, args: &[&str], timeout: StdDuration) -> BoundedRun {
        self.shows
            .push(args.iter().map(|s| s.to_string()).collect());
        self.show_timeouts.push(timeout);
        self.show.pop_front().unwrap_or_else(|| show_active(5151))
    }
    fn self_cgroup(&self) -> Option<String> {
        self.cgroup.clone()
    }
}

struct Launched {
    result: Result<WaveSpawn>,
    detached: usize,
}

fn launch_with(
    host: &mut WaveHost,
    setting: WaveUnitSetting,
    invoker: Option<&str>,
    repo: &str,
    log: &str,
    argv: &[String],
) -> Launched {
    let mut detached = 0usize;
    let mut launcher = || -> Result<(u32, Option<String>)> {
        detached += 1;
        Ok((9090, Some("start-9090".to_string())))
    };
    let w = WaveLaunch {
        setting,
        invoker,
        repo,
        exe: EXE,
        path_env: PATH_ENV,
        argv,
        log,
        now: now(),
        tick_pid: TICK_PID,
        runtime_max_secs: cfg_on().wave_runtime_max_secs(),
    };
    let identity = |pid: u32| Some(format!("start-{pid}"));
    let result = launch_wave_isolated(host, &w, &mut launcher, &identity);
    Launched { result, detached }
}

fn launch(host: &mut WaveHost, invoker: Option<&str>) -> Launched {
    launch_with(
        host,
        WaveUnitSetting::Auto,
        invoker,
        REPO,
        LOG,
        &wave_argv(),
    )
}

fn systemd_launch_args() -> Vec<String> {
    let mut host = WaveHost::systemd();
    let l = launch(&mut host, Some("systemd"));
    assert!(l.result.is_ok() && l.detached == 0);
    host.runs.pop().expect("systemd-run was called")
}

/// Split the `systemd-run` argv at `--`: (options, command).
fn split_at_dashdash(args: &[String]) -> (&[String], &[String]) {
    let i = args.iter().position(|a| a == "--").expect("a `--`");
    (&args[..i], &args[i + 1..])
}

/// The values following each `flag` among the options.
fn values_of<'a>(opts: &'a [String], flag: &str) -> Vec<&'a str> {
    opts.windows(2)
        .filter(|w| w[0] == flag)
        .map(|w| w[1].as_str())
        .collect()
}

// ---------------------------------------------------------------------------
// Authority boundary (B-a .. B-e)
// ---------------------------------------------------------------------------

#[test]
fn wave_unit_argv_has_no_timer_or_restart_flags() {
    let args = systemd_launch_args();
    let (opts, _) = split_at_dashdash(&args);
    // B-a/B-b: every option is on the allowlist; `-E`/`-p` values follow.
    const FLAGS: [&str; 6] = [
        "--user",
        "--collect",
        "--no-ask-password",
        "--quiet",
        "--service-type=exec",
        "-E",
    ];
    const PREFIXED: [&str; 3] = ["--unit=", "--working-directory=", "--description="];
    let mut i = 0;
    while i < opts.len() {
        let o = opts[i].as_str();
        if o == "-E" || o == "-p" {
            i += 2;
            continue;
        }
        assert!(
            FLAGS.contains(&o) || PREFIXED.iter().any(|p| o.starts_with(p)),
            "option {o:?} is not on the wave-unit allowlist"
        );
        i += 1;
    }
    for p in values_of(opts, "-p") {
        assert!(
            [
                "UnsetEnvironment=",
                "StandardOutput=append:",
                "StandardError=append:",
                "RuntimeMaxSec=",
                "OOMPolicy=stop",
            ]
            .iter()
            .any(|allowed| p.starts_with(allowed)),
            "property {p:?} is not on the allowlist"
        );
    }
    for banned in [
        "--on-",
        "--timer-property",
        "--path-property",
        "--socket-property",
        "Restart",
        "--remain-after-exit",
        "--scope",
    ] {
        assert!(
            !opts.iter().any(|o| o.contains(banned)),
            "`{banned}` in {opts:?}"
        );
    }
    assert!(!opts.iter().any(|o| o == "-r" || o == "-G"));
}

#[test]
fn wave_launch_only_probes_with_show_never_starts_or_enables() {
    // B-e: the only systemctl call is the read-only `show`; the fake host
    // panics on any unbounded systemctl, crontab or unit-directory access.
    for (run, shows) in [
        (exited(true, "", ""), vec![show_active(5151)]),
        (BoundedRun::TimedOut, vec![show_active(5151)]),
        (exited(false, "", "Job failed"), vec![show_not_found()]),
    ] {
        let mut host = WaveHost::systemd().with_show(shows);
        host.run = run;
        let _ = launch(&mut host, Some("systemd"));
        for s in &host.shows {
            assert_eq!(s[0], "show", "{s:?}");
            for verb in ["start", "enable", "link", "daemon-reload", "stop", "kill"] {
                assert!(!s.iter().any(|a| a == verb), "{s:?}");
            }
        }
        for r in &host.runs {
            assert_eq!(r[0], "--user");
        }
    }
}

#[test]
fn wave_argv_identical_on_unit_and_detached_paths() {
    // B-d: the unit path runs exactly the argv the detached path would.
    let argv = wave_argv();
    let args = systemd_launch_args();
    let (_, command) = split_at_dashdash(&args);
    assert_eq!(command[0], EXE);
    assert_eq!(&command[1..], argv.as_slice());
    let tmp = tempfile::tempdir().unwrap();
    let cmd = wave_command(
        Path::new(EXE),
        &argv,
        tmp.path(),
        &tmp.path().join("wave.log"),
    )
    .unwrap();
    let detached: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert_eq!(detached, argv);
    assert_eq!(cmd.get_program(), EXE);
}

#[test]
fn wave_unit_helper_has_single_call_site() {
    // B-c: one launch seam, reached from one place.
    let shift = include_str!("../shift.rs");
    assert_eq!(
        shift.matches("launch_wave_isolated(").count(),
        2,
        "def + one call"
    );
    assert_eq!(shift.matches("exec.spawn_wave(").count(), 1);
    assert_eq!(shift.matches(".systemd_run_user(").count(), 1);
    assert!(!shift.contains("pub(crate) fn launch_wave_isolated"));
    assert!(!shift.contains("pub fn launch_wave_isolated"));
    // The one call is in RealExec::spawn_wave, which only tick_core calls.
    let start = shift
        .find("fn spawn_wave(&mut self, argv: &[String], log: &Path) -> Result<WaveSpawn> {")
        .expect("RealExec::spawn_wave");
    let end = start + shift[start..].find("fn save_state").unwrap();
    assert!(shift[start..end].contains("launch_wave_isolated("));
    // No other source file calls systemd-run or the launch helper.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let body = std::fs::read_to_string(&path).unwrap();
        if name != "shift.rs" {
            assert!(!body.contains("launch_wave_isolated"), "{name}");
        }
        if name != "shift.rs" && name != "schedule_driver.rs" {
            assert!(!body.contains("systemd_run_user"), "{name}");
            assert!(!body.contains("build_wave_unit_argv"), "{name}");
        }
    }
    let driver = include_str!("../schedule_driver.rs");
    assert_eq!(
        driver.matches("systemd_run_user(").count(),
        2,
        "the trait method and the real host only"
    );
    assert_eq!(driver.matches("Command::new(\"systemd-run\")").count(), 1);
}

#[test]
fn real_host_refuses_systemd_run_under_cfg_test() {
    let mut host = RealDriverHost;
    let args = vec!["--user".to_string(), "true".to_string()];
    match host.systemd_run_user(&args, StdDuration::from_secs(1)) {
        BoundedRun::SpawnFailed(why) => assert!(why.contains("test"), "{why}"),
        other => panic!("the real host ran systemd-run under test: {other:?}"),
    }
    match host.systemctl_user_bounded(&["show", "x.service"], StdDuration::from_secs(1)) {
        BoundedRun::SpawnFailed(why) => assert!(why.contains("test"), "{why}"),
        other => panic!("the real host ran systemctl under test: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// A1: environment parity
// ---------------------------------------------------------------------------

#[test]
fn wave_unit_argv_env_is_allowlist_only_no_scrubbed_or_tmux_vars() {
    let args = systemd_launch_args();
    let (opts, _) = split_at_dashdash(&args);
    let set: Vec<(&str, &str)> = values_of(opts, "-E")
        .into_iter()
        .map(|kv| kv.split_once('=').expect("KEY=VALUE"))
        .collect();
    let set_keys: BTreeSet<&str> = set.iter().map(|(k, _)| *k).collect();
    assert_eq!(
        set_keys,
        ["PATH", "AIDA_SCHEDULE_INVOKER", "GIT_TERMINAL_PROMPT"]
            .into_iter()
            .collect()
    );
    assert!(set.contains(&("GIT_TERMINAL_PROMPT", "0")));
    assert!(set.contains(&("AIDA_SCHEDULE_INVOKER", "systemd")));
    assert!(set.contains(&("PATH", PATH_ENV)));
    let unset: BTreeSet<&str> = values_of(opts, "-p")
        .into_iter()
        .filter_map(|p| p.strip_prefix("UnsetEnvironment="))
        .collect();
    let mut want: BTreeSet<&str> = SCRUBBED_ENV.iter().copied().collect();
    want.insert("TMUX");
    want.insert("AIDA_PANES");
    assert_eq!(unset, want);
    for k in SCRUBBED_ENV.iter().chain(&["TMUX", "AIDA_PANES"]) {
        assert!(!set_keys.contains(k), "the unit sets {k}");
    }
}

#[test]
fn wave_unit_env_delta_matches_detached_path() {
    // A1 (pure): every key the detached wave removes, the unit unsets; the
    // unit's sets are exactly what the detached wave inherits from the tick.
    let tmp = tempfile::tempdir().unwrap();
    let cmd = wave_command(
        Path::new(EXE),
        &wave_argv(),
        tmp.path(),
        &tmp.path().join("wave.log"),
    )
    .unwrap();
    let detached_removed: BTreeSet<String> = cmd
        .get_envs()
        .filter(|(_, v)| v.is_none())
        .map(|(k, _)| k.to_string_lossy().to_string())
        .collect();
    let detached_set: Vec<String> = cmd
        .get_envs()
        .filter(|(_, v)| v.is_some())
        .map(|(k, _)| k.to_string_lossy().to_string())
        .collect();
    assert!(detached_set.is_empty(), "the detached wave sets nothing");
    let unit_unset: BTreeSet<String> = wave_unit_unset_env()
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(
        detached_removed,
        SCRUBBED_ENV.iter().map(|s| s.to_string()).collect()
    );
    assert!(detached_removed.is_subset(&unit_unset));
    let extra: BTreeSet<String> = unit_unset.difference(&detached_removed).cloned().collect();
    assert_eq!(
        extra,
        ["TMUX", "AIDA_PANES"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    for (k, _) in wave_unit_set_env(PATH_ENV) {
        assert!(!unit_unset.contains(&k));
        assert!(!SCRUBBED_ENV.contains(&k.as_str()));
    }
}

// ---------------------------------------------------------------------------
// Selection gate (A4) and the paths that never attempt a unit
// ---------------------------------------------------------------------------

#[test]
fn cron_invoker_never_calls_systemd_run() {
    let mut host = WaveHost::systemd();
    let l = launch(&mut host, Some("cron"));
    let spawn = l.result.unwrap();
    assert_eq!(l.detached, 1);
    assert!(host.runs.is_empty() && host.shows.is_empty());
    assert_eq!(spawn.pid, Some(9090));
    assert_eq!(spawn.unit, None);
    assert_eq!(spawn.isolation, WaveIsolation::Detached { reason: None });
}

#[test]
fn manual_invoker_uses_detached_path() {
    for invoker in [None, Some("manual"), Some("hook"), Some("")] {
        let mut host = WaveHost::systemd();
        let l = launch(&mut host, invoker);
        assert_eq!(l.detached, 1, "{invoker:?}");
        assert!(host.runs.is_empty());
        assert_eq!(
            l.result.unwrap().isolation,
            WaveIsolation::Detached { reason: None }
        );
    }
}

#[test]
fn non_linux_host_uses_detached_path_without_attempt() {
    let mut host = WaveHost::systemd();
    host.supported = false;
    let l = launch(&mut host, Some("systemd"));
    assert_eq!(l.detached, 1);
    assert!(host.runs.is_empty() && host.shows.is_empty());
    assert_eq!(
        l.result.unwrap().isolation,
        WaveIsolation::Detached { reason: None }
    );
}

#[test]
fn wave_unit_off_uses_detached_path() {
    let mut host = WaveHost::systemd();
    let l = launch_with(
        &mut host,
        WaveUnitSetting::Off,
        Some("systemd"),
        REPO,
        LOG,
        &wave_argv(),
    );
    assert_eq!(l.detached, 1);
    assert!(host.runs.is_empty());
    assert_eq!(
        l.result.unwrap().isolation,
        WaveIsolation::Detached { reason: None }
    );
}

#[test]
fn systemd_invoker_outside_the_tick_unit_cgroup_stays_detached() {
    // A4: a manual or agent tick that exports AIDA_SCHEDULE_INVOKER=systemd
    // is not inside this repo's tick unit, so it gets no unit.
    let cgroups: Vec<Option<String>> = vec![
        None,
        Some(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/vte-spawn-1.scope\n"
                .to_string(),
        ),
        // Another repo's tick unit.
        Some(tick_cgroup("/repo/other")),
        // Our stem as a prefix only.
        Some(format!(
            "0::/user.slice/user@1000.service/app.slice/{}.service.d\n",
            unit_stem(REPO)
        )),
    ];
    for cgroup in cgroups {
        let mut host = WaveHost::systemd();
        host.cgroup = cgroup.clone();
        let l = launch(&mut host, Some("systemd"));
        assert_eq!(l.detached, 1, "{cgroup:?}");
        assert!(host.runs.is_empty(), "{cgroup:?}");
        match l.result.unwrap().isolation {
            WaveIsolation::Detached { reason: Some(r) } => {
                assert!(r.contains(&unit_stem(REPO)), "{r}")
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn cgroup_in_unit_matches_the_unit_cgroup_only() {
    let unit = format!("{}.service", unit_stem(REPO));
    assert!(cgroup_in_unit(&tick_cgroup(REPO), &unit));
    // cgroup v1 lines too.
    assert!(cgroup_in_unit(
        &format!("1:name=systemd:/user.slice/user@1000.service/{unit}\n0::/x\n"),
        &unit
    ));
    assert!(!cgroup_in_unit(
        "0::/user.slice/user@1000.service/init.scope\n",
        &unit
    ));
    assert!(!cgroup_in_unit(&format!("0::/{unit}/sub\n"), &unit));
    assert!(!cgroup_in_unit("", &unit));
}

// ---------------------------------------------------------------------------
// A6: specifier and variable characters fall back, never escaped
// ---------------------------------------------------------------------------

#[test]
fn percent_or_dollar_in_launch_values_falls_back_detached() {
    let argv = wave_argv();
    let cases: Vec<(String, String, Vec<String>)> = vec![
        ("/repo/a%b".into(), LOG.into(), argv.clone()),
        (
            REPO.into(),
            "/repo/aida/.aida/$HOME.log".into(),
            argv.clone(),
        ),
        (REPO.into(), LOG.into(), {
            let mut a = argv.clone();
            a.push("--note=%h".into());
            a
        }),
    ];
    for (repo, log, argv) in cases {
        let mut host = WaveHost::systemd();
        host.cgroup = Some(tick_cgroup(&repo));
        let l = launch_with(
            &mut host,
            WaveUnitSetting::Auto,
            Some("systemd"),
            &repo,
            &log,
            &argv,
        );
        assert_eq!(l.detached, 1, "{repo} {log}");
        assert!(host.runs.is_empty(), "never escaped in place");
        match l.result.unwrap().isolation {
            WaveIsolation::Detached { reason: Some(r) } => {
                assert!(r.contains("'%' or '$'"), "{r}")
            }
            other => panic!("{other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// A7: the unit's properties and max_runtime
// ---------------------------------------------------------------------------

#[test]
fn wave_unit_properties_are_exactly_the_amended_set() {
    let args = systemd_launch_args();
    let (opts, _) = split_at_dashdash(&args);
    let props: Vec<&str> = values_of(opts, "-p")
        .into_iter()
        .filter(|p| !p.starts_with("UnsetEnvironment="))
        .collect();
    let want = [
        format!("StandardOutput=append:{LOG}"),
        format!("StandardError=append:{LOG}"),
        "RuntimeMaxSec=12600".to_string(),
        "OOMPolicy=stop".to_string(),
    ];
    assert_eq!(props, want.iter().map(String::as_str).collect::<Vec<_>>());
    let unit = wave_unit_name(REPO, now(), TICK_PID);
    assert!(opts.contains(&format!("--unit={unit}")));
    assert!(opts.contains(&format!("--working-directory={REPO}")));
    assert!(opts.contains(&format!(
        "--description=AIDA drain wave batch:shift-20260924-2210 for {REPO}"
    )));
    for f in [
        "--collect",
        "--service-type=exec",
        "--no-ask-password",
        "--quiet",
    ] {
        assert!(opts.iter().any(|o| o == f), "{f}");
    }
    assert!(!opts
        .iter()
        .any(|o| o.contains("Memory") || o.contains("Weight")));
}

#[test]
fn runtime_max_is_max_runtime_plus_thirty_minutes() {
    assert_eq!(cfg_on().wave_runtime_max_secs(), Some(3 * 3600 + 1800));
    let c = cfg_with("[shift]\nmax_runtime = \"90m\"\n");
    assert_eq!(c.wave_runtime_max_secs(), Some(90 * 60 + 1800));
}

#[test]
fn unparseable_max_runtime_refuses_launch_on_both_paths() {
    for committed in [
        "[shift]\nmax_runtime = \"soon\"\n",
        "[shift]\nmax_runtime = 3\n",
        "[shift]\nmax_runtime = \"soon\"\nwave_unit = \"off\"\n",
    ] {
        let cfg = cfg_with(committed);
        assert!(cfg.max_runtime_error.is_some(), "{committed}");
        assert_eq!(cfg.wave_runtime_max_secs(), None);
        let mut exec = TickExec::new(WaveHost::systemd());
        let mut state = ShiftState::default();
        let r = tick_core(&cfg, &probes(), &mut state, &ctx(), &mut exec).unwrap();
        let g = r.guards.iter().find(|g| g.name == "max-runtime").unwrap();
        assert!(!g.pass, "{committed}: {g:?}");
        assert!(r.launched.is_none() && exec.detached == 0 && exec.host.runs.is_empty());
    }
    let r = tick_core(
        &cfg_on(),
        &probes(),
        &mut ShiftState::default(),
        &ctx(),
        &mut TickExec::new(WaveHost::systemd()),
    )
    .unwrap();
    assert!(r.guards.iter().any(|g| g.name == "max-runtime" && g.pass));
}

#[test]
fn wave_unit_switch_defaults_to_auto_and_parses_off() {
    assert_eq!(cfg_on().wave_unit, WaveUnitSetting::Auto);
    assert_eq!(
        cfg_with("[shift]\nwave_unit = \"off\"\n").wave_unit,
        WaveUnitSetting::Off
    );
    assert_eq!(
        cfg_with("[shift]\nwave_unit = false\n").wave_unit,
        WaveUnitSetting::Off
    );
    assert_eq!(
        cfg_with("[shift]\nwave_unit = \"auto\"\n").wave_unit,
        WaveUnitSetting::Auto
    );
    let local: toml::Value = toml::from_str(&format!(
        "[repo.\"{REPO}\"]\nenabled = true\nwave_unit = \"off\"\n"
    ))
    .unwrap();
    assert_eq!(
        build_config(None, Some(&local), REPO, "L").wave_unit,
        WaveUnitSetting::Off
    );
}

// ---------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------

#[test]
fn wave_unit_name_is_unit_safe_and_repo_prefixed() {
    let name = wave_unit_name(REPO, now(), TICK_PID);
    let hex = unit_stem(REPO).trim_start_matches("aida-tick-").to_string();
    assert_eq!(name, format!("aida-wave-{hex}-20260924-221000-777.service"));
    let stem = name.trim_end_matches(".service");
    assert!(stem
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
    assert!(!name.contains('%'));
    assert_ne!(
        name,
        wave_unit_name("/repo/other", now(), TICK_PID),
        "per repo"
    );
    // The tick unit name did not change.
    assert!(unit_stem(REPO).starts_with("aida-tick-") && unit_stem(REPO).len() == 18);
}

// ---------------------------------------------------------------------------
// Failure classes: fallback only when nothing started
// ---------------------------------------------------------------------------

#[test]
fn successful_start_records_unit_and_main_pid() {
    let mut host = WaveHost::systemd().with_show(vec![show_active(5151)]);
    let l = launch(&mut host, Some("systemd"));
    let spawn = l.result.unwrap();
    let unit = wave_unit_name(REPO, now(), TICK_PID);
    assert_eq!(l.detached, 0);
    assert_eq!(spawn.pid, Some(5151));
    assert_eq!(spawn.pid_start.as_deref(), Some("start-5151"));
    assert_eq!(spawn.unit.as_deref(), Some(unit.as_str()));
    assert_eq!(
        spawn.isolation,
        WaveIsolation::Unit {
            name: unit.clone(),
            note: None
        }
    );
    assert_eq!(host.shows[0].last().unwrap(), &unit);
    // A5: both calls are bounded, together inside the launch reserve.
    assert_eq!(host.run_timeouts, vec![WAVE_UNIT_RUN_TIMEOUT]);
    assert_eq!(host.show_timeouts, vec![WAVE_UNIT_PROBE_TIMEOUT]);
    assert!(WAVE_UNIT_RUN_TIMEOUT + WAVE_UNIT_PROBE_TIMEOUT <= StdDuration::from_secs(30));
}

#[test]
fn no_bus_failure_falls_back_and_reports_reason() {
    let mut host = WaveHost::systemd();
    host.run = exited(false, "", "Failed to connect to bus: No medium found");
    let l = launch(&mut host, Some("systemd"));
    assert_eq!(l.detached, 1);
    assert!(
        host.shows.is_empty(),
        "provably not started: no probe needed"
    );
    let spawn = l.result.unwrap();
    assert_eq!(spawn.pid, Some(9090));
    match &spawn.isolation {
        WaveIsolation::Detached { reason: Some(r) } => {
            assert!(
                r.contains("user manager") && r.contains("No medium found"),
                "{r}"
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(spawn.isolation.note().unwrap().contains("detached"));
}

#[test]
fn no_bus_text_with_zero_exit_is_not_a_fallback() {
    // A no-bus message counts only with a non-zero exit.
    let out = exited(true, "", "Failed to connect to bus: No medium found");
    assert_eq!(classify_systemd_run(&out), Ok(()));
}

#[test]
fn missing_systemd_run_binary_falls_back() {
    let mut host = WaveHost::systemd();
    host.run = BoundedRun::SpawnFailed("No such file or directory (os error 2)".into());
    let l = launch(&mut host, Some("systemd"));
    assert_eq!(l.detached, 1);
    assert!(host.shows.is_empty());
    match l.result.unwrap().isolation {
        WaveIsolation::Detached { reason: Some(r) } => assert!(r.contains("os error 2"), "{r}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn timeout_is_ambiguous_never_not_started() {
    assert!(matches!(
        classify_systemd_run(&BoundedRun::TimedOut),
        Err(RunFailure::Ambiguous(_))
    ));
    let mut host = WaveHost::systemd().with_show(vec![BoundedRun::TimedOut]);
    host.run = BoundedRun::TimedOut;
    let l = launch(&mut host, Some("systemd"));
    assert_eq!(l.detached, 0, "a timeout never takes the fallback blind");
    assert!(l.result.is_err());
}

#[test]
fn ambiguous_failure_with_active_unit_adopts_pid_no_second_spawn() {
    for run in [
        BoundedRun::TimedOut,
        exited(false, "", "Job for aida-wave.service failed"),
        exited(false, "", ""),
    ] {
        let mut host = WaveHost::systemd().with_show(vec![show_active(6161)]);
        host.run = run.clone();
        let l = launch(&mut host, Some("systemd"));
        assert_eq!(l.detached, 0, "{run:?}");
        assert_eq!(host.runs.len(), 1, "one systemd-run, never a retry");
        let spawn = l.result.unwrap();
        assert_eq!(spawn.pid, Some(6161));
        assert!(spawn.unit.is_some());
        match spawn.isolation {
            WaveIsolation::Unit { note: Some(n), .. } => assert!(n.contains("adopted"), "{n}"),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn ambiguous_failure_with_unknown_unit_that_systemd_does_not_know_falls_back() {
    let mut host = WaveHost::systemd().with_show(vec![show_not_found()]);
    host.run = exited(false, "", "Job failed");
    let l = launch(&mut host, Some("systemd"));
    assert_eq!(l.detached, 1);
    match l.result.unwrap().isolation {
        WaveIsolation::Detached { reason: Some(r) } => {
            assert!(
                r.contains("Job failed") && r.contains("does not know"),
                "{r}"
            )
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn ambiguous_failure_with_unknown_probe_keeps_intent_and_errs() {
    for show in [
        BoundedRun::TimedOut,
        exited(false, "", "Failed to connect to bus"),
        exited(
            true,
            "LoadState=loaded\nActiveState=failed\nMainPID=0\n",
            "",
        ),
    ] {
        let mut host = WaveHost::systemd().with_show(vec![show.clone()]);
        host.run = BoundedRun::TimedOut;
        let mut exec = TickExec::new(host);
        let mut state = ShiftState::default();
        let err = tick_core(&cfg_on(), &probes(), &mut state, &ctx(), &mut exec)
            .expect_err("an ambiguous launch is an error (CronJobFailed)");
        assert!(format!("{err:#}").contains("left pending"), "{err:#}");
        assert_eq!(exec.detached, 0, "{show:?}: never a second launch");
        // The intent was saved before the spawn; nothing after it.
        assert_eq!(exec.saves.len(), 1);
        let intent = exec.saves[0].waves.last().unwrap();
        assert!(intent.pid.is_none() && intent.unit.is_none());
        assert!(
            state.pending_intent().is_some(),
            "the next tick adopts or reuses it"
        );
        assert!(exec.events.is_empty() && exec.notifies == 0);
    }
}

#[test]
fn unit_exists_collision_never_falls_back() {
    let unit = wave_unit_name(REPO, now(), TICK_PID);
    let exists = exited(
        false,
        "",
        &format!("Failed to start transient service unit: Unit {unit} already exists."),
    );
    assert!(matches!(
        classify_systemd_run(&exists),
        Err(RunFailure::Exists(_))
    ));
    // Not found / inactive / unknown: an error, never a detached copy.
    for show in [
        show_not_found(),
        exited(
            true,
            "LoadState=loaded\nActiveState=inactive\nMainPID=0\n",
            "",
        ),
        BoundedRun::TimedOut,
    ] {
        let mut host = WaveHost::systemd().with_show(vec![show.clone()]);
        host.run = exists.clone();
        let l = launch(&mut host, Some("systemd"));
        assert_eq!(l.detached, 0, "{show:?}");
        assert!(l.result.is_err(), "{show:?}");
    }
    // Active: adopted, still no second copy.
    let mut host = WaveHost::systemd().with_show(vec![show_active(7171)]);
    host.run = exists;
    let l = launch(&mut host, Some("systemd"));
    assert_eq!(l.detached, 0);
    assert_eq!(l.result.unwrap().pid, Some(7171));
}

#[test]
fn classify_unit_show_reads_the_three_properties() {
    assert_eq!(
        classify_unit_show(&show_active(12)),
        UnitProbe::Active { main_pid: Some(12) }
    );
    assert_eq!(
        classify_unit_show(&show_active(0)),
        UnitProbe::Active { main_pid: None }
    );
    assert_eq!(classify_unit_show(&show_not_found()), UnitProbe::NotFound);
    assert_eq!(
        classify_unit_show(&exited(
            true,
            "MainPID=0\nLoadState=loaded\nActiveState=failed\n",
            ""
        )),
        UnitProbe::Inactive("failed".into())
    );
    assert!(matches!(
        classify_unit_show(&exited(true, "", "")),
        UnitProbe::Unknown(_)
    ));
    assert!(matches!(
        classify_unit_show(&BoundedRun::TimedOut),
        UnitProbe::Unknown(_)
    ));
}

// ---------------------------------------------------------------------------
// A2: a unit with MainPID=0 still counts as launched
// ---------------------------------------------------------------------------

#[test]
fn mainpid_zero_after_start_records_unit_and_settles_next_tick() {
    let host = WaveHost::systemd().with_show(vec![show_not_found()]);
    let mut exec = TickExec::new(host);
    let mut state = ShiftState::default();
    let r = tick_core(&cfg_on(), &probes(), &mut state, &ctx(), &mut exec).unwrap();
    let unit = wave_unit_name(REPO, now(), TICK_PID);
    let launched = r.launched.expect("launched");
    assert_eq!(launched.pid, 0);
    assert_eq!(launched.unit.as_deref(), Some(unit.as_str()));
    assert!(launched.isolation_note.is_some());
    assert_eq!(exec.detached, 0);
    // A3: the intent save has no unit; the post-spawn save has it.
    assert!(exec.saves[0].waves.last().unwrap().unit.is_none());
    let wave = state.waves.last().unwrap();
    assert_eq!(wave.unit.as_deref(), Some(unit.as_str()));
    assert!(wave.pid.is_none() && wave.launched());
    assert!(exec.saves[1].waves.last().unwrap().unit.is_some());
    // It counts toward the per-spec wave cap like any launch.
    assert_eq!(state.spec_waves.get("TASK-1").map(Vec::len), Some(1));
    // The event carries the unit.
    match exec.events.last() {
        Some(EventKind::ShiftTick {
            launched: Some(l), ..
        }) => assert_eq!(l.unit.as_deref(), Some(unit.as_str())),
        other => panic!("{other:?}"),
    }
    // Next tick: a unit with no pid is not alive, so it settles.
    assert!(!last_wave_is_alive(&state));
    let mut p = probes();
    p.last_wave_alive = last_wave_is_alive(&state);
    let later = TickCtx {
        now: now() + Duration::minutes(10),
        ..ctx()
    };
    let mut exec2 = TickExec::new(WaveHost::systemd());
    tick_core(&cfg_on(), &p, &mut state, &later, &mut exec2).unwrap();
    let first = state
        .waves
        .iter()
        .find(|w| w.unit.as_deref() == Some(unit.as_str()))
        .unwrap();
    assert!(first.outcome.is_some(), "settled");
}

#[test]
fn mainpid_zero_counts_toward_daily_cap_and_is_never_relaunched() {
    let at = now() - Duration::hours(1);
    let unit_only = WaveRecord {
        batch: shift_batch_name(at),
        specs: vec!["TASK-1".into()],
        at,
        argv: Vec::new(),
        pid: None,
        pid_start: None,
        log: None,
        unit: Some(wave_unit_name(REPO, at, 5)),
        outcome: None,
    };
    let state = ShiftState {
        waves: vec![unit_only.clone()],
        ..Default::default()
    };
    assert_eq!(state.waves_in_day(now()), 1);
    assert!(state.pending_intent().is_none(), "never a reusable intent");
    assert_eq!(state.last_launched(), Some(&unit_only));
    // With a cap of one, the unit-only wave fills it.
    let capped = cfg_with("[shift]\nmax_waves_per_day = 1\n");
    let mut s = state.clone();
    let mut exec = TickExec::new(WaveHost::systemd());
    let r = tick_core(&capped, &probes(), &mut s, &ctx(), &mut exec).unwrap();
    assert!(!r.guards.iter().find(|g| g.name == "wave-cap").unwrap().pass);
    assert!(r.launched.is_none() && exec.host.runs.is_empty() && exec.detached == 0);
    // Under the cap the next wave is a NEW batch, never a relaunch of it.
    let mut s = state.clone();
    let mut exec = TickExec::new(WaveHost::systemd());
    let r = tick_core(&cfg_on(), &probes(), &mut s, &ctx(), &mut exec).unwrap();
    assert!(!r.reused_batch);
    assert_ne!(r.batch.as_deref(), Some(unit_only.batch.as_str()));
    // The unit-only wave was settled first (not alive), then kept.
    assert!(s.waves[0].outcome.is_some());
    assert_eq!(s.waves.len(), 2);
}

#[test]
fn legacy_state_without_unit_field_loads() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("shift-state.json");
    std::fs::write(
        &path,
        r#"{"version":1,"waves":[{"batch":"shift-20260924-2210","specs":["TASK-1"],
            "at":"2026-09-24T22:10:00Z","argv":[],"pid":42,"pid_start":null,
            "log":null,"outcome":null}]}"#,
    )
    .unwrap();
    let state = load_state(&path).unwrap();
    let w = &state.waves[0];
    assert_eq!(w.pid, Some(42));
    assert_eq!(w.unit, None);
    assert!(w.launched());
    // A detached record still serializes without the field.
    let body = serde_json::to_string(w).unwrap();
    assert!(!body.contains("\"unit\""), "{body}");
    // And an old launch event without `unit` still parses.
    let ev: ShiftLaunch =
        serde_json::from_str(r#"{"batch":"shift-1","specs":[],"pid":7,"argv":[]}"#).unwrap();
    assert_eq!(ev.unit, None);
    assert_eq!(ev.isolation_note, None);
}

#[test]
fn render_report_names_the_unit_and_a_fallback_reason() {
    let base = TickReport {
        launched: Some(ShiftLaunch {
            batch: "shift-1".into(),
            specs: vec!["TASK-1".into()],
            pid: 5151,
            argv: Vec::new(),
            unit: Some("aida-wave-x.service".into()),
            isolation_note: None,
        }),
        wave_isolation: Some(WaveIsolation::Unit {
            name: "aida-wave-x.service".into(),
            note: None,
        }),
        ..Default::default()
    };
    let out = render_report(&base);
    assert!(
        out.contains("launched batch:shift-1 (1 specs, pid 5151)"),
        "{out}"
    );
    assert!(
        out.contains("journalctl --user -u aida-wave-x.service"),
        "{out}"
    );
    let fallback = TickReport {
        wave_isolation: Some(WaveIsolation::Detached {
            reason: Some("systemd-run could not be started (gone)".into()),
        }),
        ..base.clone()
    };
    let out = render_report(&fallback);
    assert!(
        out.contains("launched detached, not in its own unit: systemd-run could not be started"),
        "{out}"
    );
    let plain = TickReport {
        wave_isolation: Some(WaveIsolation::Detached { reason: None }),
        ..base
    };
    assert!(!render_report(&plain).contains("wave unit"));
}

// ---------------------------------------------------------------------------
// tick_core harness: the production launch helper over a fake host
// ---------------------------------------------------------------------------

fn probes() -> Probes {
    let candidates = vec![Candidate {
        spec: "TASK-1".to_string(),
        status: RequirementStatus::Approved,
        execution_mode: Some(ExecutionMode::Drain),
        req_type: "Story".to_string(),
        tags: Vec::new(),
        merge_held: false,
    }];
    Probes {
        lock: LockView::Free,
        lock_holder: None,
        foreign_claim: None,
        no_human_ack: Some("~/.aida/no-human-acknowledged".to_string()),
        budget: Some(crate::runaway_seats::BudgetEvidence {
            spent_24h: 1_000_000_000,
            last_run_at: Some(now() - Duration::minutes(5)),
            last_verdict: Some("ok".to_string()),
        }),
        vendor: "claude".to_string(),
        vendor_error: None,
        watchdog_failures: 0,
        load: Some((1.0, 8)),
        memory: Some((16 << 30, 4 << 30)),
        disk: Some((true, "1 volume(s) have headroom".to_string())),
        queue_user: "joe".to_string(),
        queue_fingerprint: vec!["TASK-1".to_string()],
        candidates,
        last_wave_alive: false,
        queue_drained: Vec::new(),
        finished_specs: BTreeSet::new(),
        parked: Vec::new(),
        held: BTreeSet::new(),
        redrive_history: Ok(events::RedriveHistory::default()),
        mail: Ok(BTreeMap::new()),
        mail_known: BTreeSet::new(),
    }
}

fn ctx() -> TickCtx {
    TickCtx {
        now: now(),
        dry_run: false,
        optional_allowed: true,
        launch_allowed: true,
        state_error: None,
        log_path: PathBuf::from(LOG),
        clock: None,
    }
}

/// A [`ShiftExec`] whose `spawn_wave` runs the real launch helper against a
/// fake host and a counting detached launcher.
struct TickExec {
    host: WaveHost,
    detached: usize,
    saves: Vec<ShiftState>,
    events: Vec<EventKind>,
    notifies: usize,
}

impl TickExec {
    fn new(host: WaveHost) -> Self {
        Self {
            host,
            detached: 0,
            saves: Vec::new(),
            events: Vec::new(),
            notifies: 0,
        }
    }
}

impl ShiftExec for TickExec {
    fn reap(&mut self) -> usize {
        0
    }
    fn tag_batch(&mut self, _: &str, _: &[String]) -> Result<()> {
        Ok(())
    }
    fn spawn_wave(&mut self, argv: &[String], log: &Path) -> Result<WaveSpawn> {
        let log = log.display().to_string();
        let w = WaveLaunch {
            setting: WaveUnitSetting::Auto,
            invoker: Some("systemd"),
            repo: REPO,
            exe: EXE,
            path_env: PATH_ENV,
            argv,
            log: &log,
            now: now(),
            tick_pid: TICK_PID,
            runtime_max_secs: cfg_on().wave_runtime_max_secs(),
        };
        let detached = &mut self.detached;
        let mut launcher = || -> Result<(u32, Option<String>)> {
            *detached += 1;
            Ok((9090, None))
        };
        let identity = |pid: u32| Some(format!("start-{pid}"));
        // Test harness only; production has exactly one call site.
        launch_wave_isolated(&mut self.host, &w, &mut launcher, &identity)
    }
    fn save_state(&mut self, state: &ShiftState) -> Result<()> {
        self.saves.push(state.clone());
        Ok(())
    }
    fn emit(&mut self, kind: EventKind) {
        self.events.push(kind);
    }
    fn requeue(
        &mut self,
        _: &[crate::supervisor::SuperviseDecision],
        _: &crate::supervisor::QueueTarget,
    ) -> Result<crate::supervisor::RequeueOutcome> {
        Ok(Default::default())
    }
    fn reclassify(&mut self, _: &crate::supervisor::SuperviseDecision) -> Result<bool> {
        Ok(false)
    }
    fn notify(
        &mut self,
        _: &str,
        _: &str,
        _: &str,
        _: StdDuration,
    ) -> Result<crate::notify::DirectDelivery> {
        self.notifies += 1;
        Ok(crate::notify::DirectDelivery::Sent)
    }
}

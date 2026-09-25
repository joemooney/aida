//! STORY-1218 slice 1: the night-shift tick. Every test drives `tick_core`
//! through a recording [`ShiftExec`] mock, so no test can launch a drain; the
//! one process test spawns `/bin/sh` through the wave builder, never `aida`.
// trace:STORY-1218 | ai:claude

use super::*;
use chrono::TimeZone;

const REPO: &str = "/repo/aida";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 24, 22, 10, 0).unwrap()
}

fn local_on() -> toml::Value {
    toml::from_str(&format!("[repo.\"{REPO}\"]\nenabled = true\n")).unwrap()
}

fn cfg_with(committed: Option<&str>, local: Option<&str>) -> ShiftConfig {
    let committed = committed.map(|c| toml::from_str::<toml::Value>(c).unwrap());
    let local = local.map(|l| toml::from_str::<toml::Value>(l).unwrap());
    build_config(committed.as_ref(), local.as_ref(), REPO, "L")
}

fn cfg_on() -> ShiftConfig {
    build_config(None, Some(&local_on()), REPO, "L")
}

fn cand(spec: &str, mode: Option<ExecutionMode>) -> Candidate {
    Candidate {
        spec: spec.to_string(),
        status: RequirementStatus::Approved,
        execution_mode: mode,
        req_type: "Story".to_string(),
        tags: Vec::new(),
        merge_held: false,
    }
}

fn drain(spec: &str) -> Candidate {
    cand(spec, Some(ExecutionMode::Drain))
}

fn probes(candidates: Vec<Candidate>) -> Probes {
    let mut fingerprint: Vec<String> = candidates.iter().map(|c| c.spec.clone()).collect();
    fingerprint.sort();
    Probes {
        lock: LockView::Free,
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
        candidates,
        queue_fingerprint: fingerprint,
        last_wave_alive: false,
        queue_drained: Vec::new(),
        finished_specs: BTreeSet::new(),
    }
}

fn ctx() -> TickCtx {
    TickCtx {
        now: now(),
        dry_run: false,
        optional_allowed: true,
        launch_allowed: true,
        state_error: None,
        log_path: PathBuf::from("/nonexistent/shift-wave.log"),
        clock: None,
    }
}

#[derive(Default)]
struct Mock {
    calls: Vec<String>,
    tags: Vec<(String, Vec<String>)>,
    spawns: Vec<Vec<String>>,
    saves: Vec<ShiftState>,
    events: Vec<EventKind>,
    reap_count: usize,
}

impl ShiftExec for Mock {
    fn reap(&mut self) -> usize {
        self.calls.push("reap".into());
        self.reap_count
    }
    fn tag_batch(&mut self, batch: &str, specs: &[String]) -> Result<()> {
        self.calls.push("tag".into());
        self.tags.push((batch.to_string(), specs.to_vec()));
        Ok(())
    }
    fn spawn_wave(&mut self, argv: &[String], _log: &Path) -> Result<(u32, Option<String>)> {
        self.calls.push("spawn".into());
        self.spawns.push(argv.to_vec());
        Ok((4242, Some("start-4242".into())))
    }
    fn save_state(&mut self, state: &ShiftState) -> Result<()> {
        self.calls.push("save".into());
        self.saves.push(state.clone());
        Ok(())
    }
    fn emit(&mut self, kind: EventKind) {
        self.calls.push("emit".into());
        self.events.push(kind);
    }
}

fn run(cfg: &ShiftConfig, p: &Probes, state: &mut ShiftState, c: &TickCtx) -> (TickReport, Mock) {
    let mut mock = Mock::default();
    let report = tick_core(cfg, p, state, c, &mut mock).expect("a refusal is never an error");
    (report, mock)
}

fn guard<'a>(r: &'a TickReport, name: &str) -> &'a GuardVerdict {
    r.guards
        .iter()
        .find(|g| g.name == name)
        .unwrap_or_else(|| panic!("no guard {name}"))
}

fn launched_wave(at: DateTime<Utc>, specs: &[&str]) -> WaveRecord {
    WaveRecord {
        batch: shift_batch_name(at),
        specs: specs.iter().map(|s| s.to_string()).collect(),
        at,
        argv: Vec::new(),
        pid: Some(7),
        pid_start: None,
        log: None,
        outcome: None,
    }
}

// ---------------------------------------------------------------------------
// Launch and no-op
// ---------------------------------------------------------------------------

#[test]
fn shift_tick_launches_wave_when_lock_free_and_event_names_it() {
    let p = probes(vec![drain("TASK-1"), drain("TASK-2"), drain("TASK-3")]);
    let mut state = ShiftState::default();
    let (r, mock) = run(&cfg_on(), &p, &mut state, &ctx());
    let launched = r.launched.expect("every guard passes, so it launches");
    assert_eq!(launched.batch, "shift-20260924-2210");
    assert_eq!(launched.specs, vec!["TASK-1", "TASK-2", "TASK-3"]);
    assert_eq!(launched.pid, 4242);
    assert_eq!(mock.spawns.len(), 1);
    assert_eq!(
        mock.tags,
        vec![("shift-20260924-2210".to_string(), launched.specs.clone())]
    );
    // A4 order: intent saved, then tag, then spawn, then the pid recorded.
    let pos = |c: &str| mock.calls.iter().position(|x| x == c).unwrap();
    assert!(
        pos("save") < pos("tag") && pos("tag") < pos("spawn"),
        "{:?}",
        mock.calls
    );
    assert_eq!(
        mock.saves[0].waves.last().unwrap().pid,
        None,
        "intent first"
    );
    assert_eq!(
        mock.saves.last().unwrap().waves.last().unwrap().pid,
        Some(4242)
    );
    // The event names the wave.
    assert_eq!(mock.events.len(), 1);
    match &mock.events[0] {
        EventKind::ShiftTick {
            launched: Some(l), ..
        } => {
            assert_eq!(l.batch, "shift-20260924-2210");
            assert_eq!(l.specs.len(), 3);
        }
        other => panic!("expected a ShiftTick with the launch, got {other:?}"),
    }
    assert!(
        !mock.events[0].is_actionable(),
        "a routine launch is not a wake"
    );
    assert_eq!(state.waves_in_day(now()), 1);
}

#[test]
fn shift_tick_second_tick_during_wave_is_noop() {
    let p = probes(vec![drain("TASK-1"), drain("TASK-2")]);
    let mut state = ShiftState::default();
    run(&cfg_on(), &p, &mut state, &ctx());
    let mut during = p.clone();
    during.last_wave_alive = true;
    during.lock = LockView::Running(4242);
    let (r, mock) = run(&cfg_on(), &during, &mut state, &ctx());
    assert!(r.launched.is_none());
    assert!(mock.spawns.is_empty() && mock.tags.is_empty());
    assert!(!guard(&r, "wave-in-flight").pass);
    assert!(!guard(&r, "lock-free").pass);
    // The verdict set changed (a launch became a hold): one event. A third
    // identical tick is silent.
    assert_eq!(mock.events.len(), 1);
    let (_, third) = run(&cfg_on(), &during, &mut state, &ctx());
    assert!(third.events.is_empty(), "{:?}", third.events);
    assert_eq!(state.waves_in_day(now()), 1);
}

#[test]
fn shift_tick_launch_in_progress_before_lock_written_is_noop() {
    // The wave was spawned (pid recorded, alive) but has not written the
    // drain lock yet: the lock probe says free, the tick must still hold.
    let mut state = ShiftState {
        waves: vec![launched_wave(now() - Duration::seconds(20), &["TASK-1"])],
        ..Default::default()
    };
    let mut p = probes(vec![drain("TASK-2")]);
    p.last_wave_alive = true;
    let (r, mock) = run(&cfg_on(), &p, &mut state, &ctx());
    assert!(r.launched.is_none() && mock.spawns.is_empty());
    assert!(guard(&r, "lock-free").pass);
    assert!(!guard(&r, "wave-in-flight").pass);
}

#[test]
fn shift_tick_never_selects_drive_guided_operator_decide() {
    let p = probes(vec![
        cand("TASK-1", Some(ExecutionMode::Drive)),
        cand("TASK-2", Some(ExecutionMode::Guided)),
        cand("TASK-3", Some(ExecutionMode::Operator)),
        cand("TASK-4", Some(ExecutionMode::Decide)),
        cand("TASK-5", None),
    ]);
    let mut state = ShiftState::default();
    let (r, mock) = run(&cfg_on(), &p, &mut state, &ctx());
    assert!(r.specs.is_empty() && r.launched.is_none() && mock.spawns.is_empty());
    assert!(!guard(&r, "drain-mode-only").pass);
    assert_eq!(r.skipped.len(), 5);
    assert!(r
        .skipped
        .iter()
        .any(|(s, why)| s == "TASK-5" && why == "no explicit execution mode"));
}

#[test]
fn shift_selection_requires_explicit_drain_and_excludes_keystone_and_held() {
    let mut keystone = drain("TASK-1");
    keystone.tags = vec!["keystone".into()];
    let mut epic = drain("EPIC-9");
    epic.req_type = "Epic".into();
    let mut held = drain("TASK-2");
    held.merge_held = true;
    let mut done = drain("TASK-3");
    done.status = RequirementStatus::InProgress;
    let p = probes(vec![keystone, epic, held, done, drain("TASK-4")]);
    let sel = select_wave(&cfg_on(), &p.candidates, &ShiftState::default(), now());
    assert_eq!(sel.specs, vec!["TASK-4"]);
    let why: BTreeMap<_, _> = sel.skipped.iter().cloned().collect();
    assert_eq!(why["TASK-1"], "keystone-class");
    assert_eq!(why["EPIC-9"], "keystone-class");
    assert_eq!(why["TASK-2"], "live merge hold");
    assert!(why["TASK-3"].starts_with("status"));
}

#[test]
fn shift_configured_batch_with_non_drain_member_refuses() {
    let cfg = cfg_with(
        Some("[shift]\nbatch = \"night1\"\n"),
        Some(&format!("[repo.\"{REPO}\"]\nenabled = true\n")),
    );
    let mut a = drain("TASK-1");
    a.tags = vec!["batch:night1".into()];
    let mut b = cand("TASK-2", Some(ExecutionMode::Drive));
    b.tags = vec!["batch:night1".into()];
    let p = probes(vec![a.clone(), b, drain("TASK-3")]);
    let mut state = ShiftState::default();
    let (r, mock) = run(&cfg, &p, &mut state, &ctx());
    assert!(r.launched.is_none() && mock.spawns.is_empty() && mock.tags.is_empty());
    let g = guard(&r, "drain-mode-only");
    assert!(!g.pass);
    assert!(g.detail.contains("TASK-2"), "{}", g.detail);
    // A clean configured batch drains without re-tagging.
    let (ok, mock) = run(&cfg, &probes(vec![a]), &mut ShiftState::default(), &ctx());
    assert_eq!(ok.launched.unwrap().batch, "night1");
    assert!(mock.tags.is_empty());
}

#[test]
fn shift_tick_live_lock_noop_stale_lock_logs_recovered_pid_and_leaves_file() {
    let tmp = tempfile::tempdir().unwrap();
    let lock = crate::drain_lock::drain_lock_path(tmp.path());
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    std::fs::write(&lock, "{}").unwrap();

    let mut live = probes(vec![drain("TASK-1")]);
    live.lock = LockView::Running(42);
    let (r, mock) = run(&cfg_on(), &live, &mut ShiftState::default(), &ctx());
    assert!(r.launched.is_none() && mock.spawns.is_empty());

    let mut stale = probes(vec![drain("TASK-1")]);
    stale.lock = LockView::Stale(41);
    let mut state = ShiftState::default();
    let (r, mock) = run(&cfg_on(), &stale, &mut state, &ctx());
    assert_eq!(r.recovered_stale_pid, Some(41));
    assert!(r.launched.is_some(), "a stale lock does not block the wave");
    assert!(matches!(
        &mock.events[0],
        EventKind::ShiftTick {
            recovered_stale_pid: Some(41),
            ..
        }
    ));
    // The tick never touches the lock file; the wave's own acquire reclaims it.
    assert!(lock.exists());
    // Reported once, not on every tick.
    let mut again = stale.clone();
    again.last_wave_alive = true;
    let (r2, _) = run(&cfg_on(), &again, &mut state, &ctx());
    assert_eq!(r2.recovered_stale_pid, None);
}

#[test]
fn shift_cross_clone_drain_claim_blocks_launch() {
    let mut p = probes(vec![drain("TASK-1")]);
    p.foreign_claim = Some("another clone is draining".into());
    let (r, mock) = run(&cfg_on(), &p, &mut ShiftState::default(), &ctx());
    assert!(r.launched.is_none() && mock.spawns.is_empty());
    assert!(!guard(&r, "cross-clone-lock").pass);

    // The read-only probe over a real claim file.
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join(".aida-store");
    let here = tmp.path().join("clone-a");
    std::fs::create_dir_all(&here).unwrap();
    let claim_path =
        crate::coordination::lock_claim_path(&store, crate::coordination::LockKind::Drain);
    std::fs::create_dir_all(claim_path.parent().unwrap()).unwrap();
    let write = |clone: &str| {
        let body = format!(
            "scope = \"drain\"\nnode_id = \"1\"\nclone_path = \"{clone}\"\nhost = \"{}\"\npid = 77\nagent = \"queue work\"\nstarted_at = \"{t}\"\nheartbeat_at = \"{t}\"\nttl_secs = 1800\nprocess_backed = true\n",
            crate::coordination::hostname(),
            t = now().to_rfc3339()
        );
        std::fs::write(&claim_path, body).unwrap();
    };
    write("/some/other/clone");
    let alive = |_: u32, _: Option<&str>| true;
    let dead = |_: u32, _: Option<&str>| false;
    let kind = crate::coordination::LockKind::Drain;
    assert!(
        crate::coordination::live_foreign_lock_claim(&store, kind, &here, now(), alive).is_some()
    );
    assert!(
        crate::coordination::live_foreign_lock_claim(&store, kind, &here, now(), dead).is_none()
    );
    write(&here.canonicalize().unwrap().display().to_string());
    assert!(
        crate::coordination::live_foreign_lock_claim(&store, kind, &here, now(), alive).is_none(),
        "our own clone's claim is not foreign"
    );
}

// ---------------------------------------------------------------------------
// The wave command (A2, A9, Q4)
// ---------------------------------------------------------------------------

#[test]
fn shift_wave_argv_never_forces() {
    let cfg = cfg_on();
    assert_eq!(cfg.max_failures, 2, "Q4: the unattended default is 2");
    assert_eq!(cfg.wave_size, 6);
    let argv = build_wave_argv(&cfg, "shift-x", 123);
    let has = |flag: &str, val: &str| argv.windows(2).any(|w| w[0] == flag && w[1] == val);
    assert!(has("--batch", "shift-x"));
    assert!(has("--max-failures", "2"));
    assert!(has("--max-iterations", "6"));
    assert!(has("--max", "6"));
    assert!(has("--max-tokens", "123"));
    assert!(has("--max-runtime", "3h"));
    assert!(has("--role", "implementer"));
    assert!(argv.iter().any(|a| a == "--no-human=both"));
    assert!(argv.iter().any(|a| a == "--escalate-blocks"));
    assert!(argv.iter().any(|a| a == "--auto-complete"));
    for forbidden in ["--force-claim", "--steal", "--force"] {
        assert!(
            !argv.iter().any(|a| a == forbidden),
            "{forbidden} in {argv:?}"
        );
    }
    // Clamp to 1..=wave_size.
    let tiny = cfg_with(
        Some("[shift]\nwave_size = 1\nmax_failures = 5\n"),
        Some(&format!("[repo.\"{REPO}\"]\nenabled = true\n")),
    );
    assert_eq!(tiny.max_failures, 1);
    // A9: --max-tokens = min(wave budget, stop threshold - spent).
    let stop = cfg.daily_token_budget / 100 * cfg.budget_stop_pct;
    assert_eq!(wave_max_tokens(&cfg, 0), cfg.wave_token_budget);
    assert_eq!(wave_max_tokens(&cfg, stop - 10), 10);
    assert_eq!(wave_max_tokens(&cfg, stop + 10), 0);
}

#[cfg(unix)]
#[test]
fn shift_wave_env_scrubbed_and_stdio_detached() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("wave.log");
    let script =
        "cat; echo stdin-closed; echo pid=$$ sid=$(awk '{print $6}' /proc/$$/stat 2>/dev/null)";
    let args = vec!["-c".to_string(), script.to_string()];
    let mut cmd = wave_command(Path::new("/bin/sh"), &args, tmp.path(), &log).unwrap();
    let envs: Vec<(String, Option<String>)> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().to_string(),
                v.map(|v| v.to_string_lossy().to_string()),
            )
        })
        .collect();
    for key in SCRUBBED_ENV {
        assert!(
            envs.iter().any(|(k, v)| k == key && v.is_none()),
            "{key} must be removed: {envs:?}"
        );
    }
    assert!(
        !envs
            .iter()
            .any(|(k, v)| k == "AIDA_NO_HUMAN_ACKNOWLEDGED" && v.is_some()),
        "A3: the tick never acknowledges no-human itself"
    );
    assert!(
        !envs.iter().any(|(k, _)| k == "AIDA_SCHEDULE_INVOKER"),
        "kept"
    );
    // stdin is /dev/null (`cat` returns at once), output lands in the log.
    let status = cmd.spawn().unwrap().wait().unwrap();
    assert!(status.success());
    let out = std::fs::read_to_string(&log).unwrap();
    assert!(out.contains("stdin-closed"), "{out}");
    #[cfg(target_os = "linux")]
    {
        let line = out.lines().find(|l| l.starts_with("pid=")).unwrap();
        let pid = line
            .split_whitespace()
            .next()
            .unwrap()
            .trim_start_matches("pid=");
        let sid = line
            .split_whitespace()
            .nth(1)
            .unwrap()
            .trim_start_matches("sid=");
        assert_eq!(pid, sid, "the wave leads its own session: {line}");
    }
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

#[test]
fn shift_refuses_without_no_human_ack_and_never_sets_env() {
    let mut p = probes(vec![drain("TASK-1")]);
    p.no_human_ack = None;
    let (r, mock) = run(&cfg_on(), &p, &mut ShiftState::default(), &ctx());
    assert!(r.launched.is_none() && mock.spawns.is_empty());
    let g = guard(&r, "no-human-ack");
    assert!(!g.pass && g.detail.contains("aida no-human acknowledge"));
    let src = include_str!("../shift.rs");
    assert!(!src.contains(".env(\"AIDA_NO_HUMAN_ACKNOWLEDGED\""));
}

#[test]
fn shift_refusals_exit_zero_and_unreadable_state_blocks_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("shift-state.json");
    assert_eq!(
        load_state(&path).unwrap(),
        ShiftState::default(),
        "missing = fresh"
    );
    std::fs::write(&path, "{ not json").unwrap();
    let err = load_state(&path).unwrap_err();
    let mut c = ctx();
    c.state_error = Some(format!("{err:#}"));
    let (r, mock) = run(
        &cfg_on(),
        &probes(vec![drain("TASK-1")]),
        &mut ShiftState::default(),
        &c,
    );
    assert!(!guard(&r, "state-readable").pass);
    assert!(r.launched.is_none() && mock.spawns.is_empty());
    assert!(
        !mock.calls.contains(&"save".to_string()),
        "never overwrite an unreadable state"
    );
    assert!(!mock.calls.contains(&"reap".to_string()));
}

#[test]
fn shift_killed_between_tag_and_spawn_reuses_batch() {
    let earlier = now() - Duration::minutes(15);
    let mut state = ShiftState {
        waves: vec![WaveRecord {
            batch: shift_batch_name(earlier),
            specs: vec!["TASK-1".into(), "TASK-2".into()],
            at: earlier,
            argv: Vec::new(),
            pid: None,
            pid_start: None,
            log: None,
            outcome: None,
        }],
        ..Default::default()
    };
    let p = probes(vec![drain("TASK-3"), drain("TASK-1"), drain("TASK-2")]);
    let (r, mock) = run(&cfg_on(), &p, &mut state, &ctx());
    assert!(r.reused_batch);
    assert_eq!(
        mock.tags,
        vec![(
            shift_batch_name(earlier),
            vec!["TASK-1".to_string(), "TASK-2".to_string()]
        )]
    );
    assert_eq!(
        state.waves.len(),
        1,
        "the intent record is completed, not duplicated"
    );
    assert_eq!(state.waves[0].pid, Some(4242));
    assert_eq!(state.waves_in_day(now()), 1);
}

#[test]
fn shift_deadline_skips_reap_and_launch() {
    let late = TickCtx::from_clock(
        now(),
        false,
        Instant::now(),
        StdDuration::ZERO,
        PathBuf::from("/nonexistent/x.log"),
    );
    assert!(!late.optional_allowed && !late.launch_allowed);
    let mut mock = Mock {
        reap_count: 3,
        ..Default::default()
    };
    let r = tick_core(
        &cfg_on(),
        &probes(vec![drain("TASK-1")]),
        &mut ShiftState::default(),
        &late,
        &mut mock,
    )
    .unwrap();
    assert!(!mock.calls.contains(&"reap".to_string()));
    assert!(mock.spawns.is_empty() && r.launched.is_none());
    assert!(!guard(&r, "deadline").pass);
    let fresh = TickCtx::from_clock(now(), false, Instant::now(), TICK_DEADLINE, PathBuf::new());
    assert!(fresh.optional_allowed && fresh.launch_allowed);
}

#[test]
fn shift_refuses_over_token_budget() {
    let cfg = cfg_on();
    let mut p = probes(vec![drain("TASK-1")]);
    let stop = cfg.daily_token_budget / 100 * cfg.budget_stop_pct;
    p.budget.as_mut().unwrap().spent_24h = stop;
    let (r, mock) = run(&cfg, &p, &mut ShiftState::default(), &ctx());
    assert!(!guard(&r, "token-budget").pass);
    assert!(mock.spawns.is_empty());
}

#[test]
fn shift_refuses_when_budget_evidence_missing_or_stale() {
    let cases: Vec<(&str, Box<dyn Fn(&mut Probes)>)> = vec![
        ("missing", Box::new(|p: &mut Probes| p.budget = None)),
        (
            "stale",
            Box::new(|p: &mut Probes| {
                p.budget.as_mut().unwrap().last_run_at = Some(now() - Duration::minutes(61))
            }),
        ),
        (
            "no run time",
            Box::new(|p: &mut Probes| p.budget.as_mut().unwrap().last_run_at = None),
        ),
        (
            "degraded",
            Box::new(|p: &mut Probes| {
                p.budget.as_mut().unwrap().last_verdict = Some("degraded".into())
            }),
        ),
        (
            "unknown",
            Box::new(|p: &mut Probes| {
                p.budget.as_mut().unwrap().last_verdict = Some("unknown".into())
            }),
        ),
        (
            "uncovered vendor",
            Box::new(|p: &mut Probes| p.vendor = "codex".into()),
        ),
    ];
    for (name, mutate) in cases {
        let mut p = probes(vec![drain("TASK-1")]);
        mutate(&mut p);
        let (r, mock) = run(&cfg_on(), &p, &mut ShiftState::default(), &ctx());
        assert!(!guard(&r, "budget-evidence").pass, "{name}");
        assert!(mock.spawns.is_empty(), "{name}");
    }
    // An explicit local override admits an uncovered vendor, and says so.
    let cfg = cfg_with(
        None,
        Some(&format!(
            "[repo.\"{REPO}\"]\nenabled = true\nallow_uncovered_vendors = [\"codex\"]\n"
        )),
    );
    let mut p = probes(vec![drain("TASK-1")]);
    p.vendor = "codex".into();
    let (r, _) = run(&cfg, &p, &mut ShiftState::default(), &ctx());
    let g = guard(&r, "budget-evidence");
    assert!(g.pass && g.detail.contains("NOT measured"), "{}", g.detail);
    // The override is local-only: the committed config cannot grant it.
    let committed = cfg_with(
        Some("[shift]\nallow_uncovered_vendors = [\"codex\"]\n"),
        Some(&format!("[repo.\"{REPO}\"]\nenabled = true\n")),
    );
    assert!(committed.allow_uncovered_vendors.is_empty());
}

#[test]
fn shift_refuses_after_runaway_watchdog_trip() {
    let mut p = probes(vec![drain("TASK-1")]);
    p.watchdog_failures = 1;
    let (r, mock) = run(&cfg_on(), &p, &mut ShiftState::default(), &ctx());
    assert!(!guard(&r, "watchdog-quiet").pass);
    assert!(mock.spawns.is_empty());
}

#[test]
fn shift_refuses_over_load_mem_disk_ceiling() {
    let cases: Vec<(&str, &str, Box<dyn Fn(&mut Probes)>)> = vec![
        (
            "load-ceiling",
            "high",
            Box::new(|p: &mut Probes| p.load = Some((20.0, 8))),
        ),
        (
            "load-ceiling",
            "unreadable",
            Box::new(|p: &mut Probes| p.load = None),
        ),
        (
            "load-ceiling",
            "no cpus",
            Box::new(|p: &mut Probes| p.load = Some((0.1, 0))),
        ),
        (
            "mem-ceiling",
            "low",
            Box::new(|p: &mut Probes| p.memory = Some((1 << 30, 4 << 30))),
        ),
        (
            "mem-ceiling",
            "unreadable",
            Box::new(|p: &mut Probes| p.memory = None),
        ),
        (
            "disk-headroom",
            "low",
            Box::new(|p: &mut Probes| p.disk = Some((false, "2 GiB free".into()))),
        ),
        (
            "disk-headroom",
            "unreadable",
            Box::new(|p: &mut Probes| p.disk = None),
        ),
    ];
    for (name, case, mutate) in cases {
        let mut p = probes(vec![drain("TASK-1")]);
        mutate(&mut p);
        let (r, mock) = run(&cfg_on(), &p, &mut ShiftState::default(), &ctx());
        assert!(!guard(&r, name).pass, "{name} {case}");
        assert!(mock.spawns.is_empty(), "{name} {case}");
    }
}

// ---------------------------------------------------------------------------
// Circuit breakers (A8)
// ---------------------------------------------------------------------------

#[test]
fn shift_dead_wave_without_queue_drained_is_zero_progress() {
    let at = now() - Duration::hours(2);
    let mut state = ShiftState {
        waves: vec![launched_wave(at, &["TASK-1"])],
        ..Default::default()
    };
    let p = probes(vec![]);
    run(&cfg_on(), &p, &mut state, &ctx());
    let o = state.waves[0]
        .outcome
        .clone()
        .expect("settled once the pid is dead");
    assert!(
        !o.progress,
        "no QueueDrained is zero progress, never unknown"
    );
    assert_eq!(state.consecutive_zero_progress, 1);

    let mut state = ShiftState {
        waves: vec![launched_wave(at, &["TASK-1"])],
        consecutive_zero_progress: 1,
        ..Default::default()
    };
    let mut p = probes(vec![]);
    p.queue_drained = vec![
        (at - Duration::hours(1), 5, 0),
        (at + Duration::minutes(40), 2, 1),
    ];
    p.finished_specs.insert("TASK-1".into());
    state.spec_waves.insert("TASK-1".into(), vec![at]);
    run(&cfg_on(), &p, &mut state, &ctx());
    let o = state.waves[0].outcome.clone().unwrap();
    assert!(o.progress);
    assert_eq!(
        (o.shipped, o.shelved),
        (2, 1),
        "only drains after the launch count"
    );
    assert_eq!(state.consecutive_zero_progress, 0);
    assert!(
        !state.spec_waves.contains_key("TASK-1"),
        "a finished spec leaves the cap"
    );
}

#[test]
fn shift_no_progress_wave_not_relaunched() {
    let at = now() - Duration::hours(2);
    let mut state = ShiftState {
        waves: vec![
            WaveRecord {
                outcome: Some(WaveOutcome {
                    settled_at: at,
                    shipped: 0,
                    shelved: 0,
                    progress: false,
                }),
                ..launched_wave(at - Duration::hours(2), &["TASK-1"])
            },
            launched_wave(at, &["TASK-1"]),
        ],
        consecutive_zero_progress: 1,
        ..Default::default()
    };
    let p = probes(vec![drain("TASK-2")]);
    let (r, mock) = run(&cfg_on(), &p, &mut state, &ctx());
    assert!(
        r.breaker.is_some(),
        "the second zero-progress wave trips it"
    );
    assert!(r.launched.is_none() && mock.spawns.is_empty());
    assert!(!guard(&r, "no-progress").pass);
    assert_eq!(mock.events.len(), 1);
    assert!(
        mock.events[0].is_actionable(),
        "the trip is the one notification"
    );

    // Later ticks stay stopped and quiet.
    let (r2, mock2) = run(&cfg_on(), &p, &mut state, &ctx());
    assert!(r2.launched.is_none() && r2.breaker.is_none());
    assert!(mock2.events.is_empty(), "{:?}", mock2.events);

    // A human change to the queue resumes launching.
    let changed = probes(vec![drain("TASK-2"), drain("TASK-9")]);
    let (r3, _) = run(&cfg_on(), &changed, &mut state, &ctx());
    assert!(r3.launched.is_some());

    // So does `aida shift resume`.
    let tmp = tempfile::tempdir().unwrap();
    let stopped = ShiftState {
        breaker: Some(Breaker {
            tripped_at: now(),
            reason: "2 consecutive shift waves made no progress".into(),
            queue_fingerprint: vec![],
        }),
        consecutive_zero_progress: 2,
        ..Default::default()
    };
    save_state_to(&state_path(tmp.path()), &stopped).unwrap();
    let mut yes = |_: &str| Ok(true);
    let mut human = Operator {
        stdin_tty: true,
        agent_mode: false,
        confirm: &mut yes,
    };
    resume_command(tmp.path(), &mut human).unwrap();
    let resumed = load_state(&state_path(tmp.path())).unwrap();
    assert!(resumed.breaker.is_none());
    assert_eq!(resumed.consecutive_zero_progress, 0);
}

#[test]
fn shift_spec_in_two_waves_excluded_and_escalated_once() {
    let mut state = ShiftState::default();
    state.spec_waves.insert(
        "TASK-1".into(),
        vec![now() - Duration::hours(5), now() - Duration::hours(1)],
    );
    let p = probes(vec![drain("TASK-1"), drain("TASK-2")]);
    let (r, mock) = run(&cfg_on(), &p, &mut state, &ctx());
    assert_eq!(r.escalated, vec!["TASK-1"]);
    assert_eq!(r.launched.unwrap().specs, vec!["TASK-2"]);
    assert!(mock.events[0].is_actionable());
    let mut during = p.clone();
    during.last_wave_alive = true;
    let (r2, _) = run(&cfg_on(), &during, &mut state, &ctx());
    assert!(r2.escalated.is_empty(), "escalated once, not every tick");
}

#[test]
fn shift_max_waves_per_day_counted_from_state() {
    let settled = |at: DateTime<Utc>| WaveRecord {
        outcome: Some(WaveOutcome {
            settled_at: at,
            shipped: 1,
            shelved: 0,
            progress: true,
        }),
        ..launched_wave(at, &["TASK-0"])
    };
    let mut state = ShiftState {
        waves: (1..=8)
            .map(|h| settled(now() - Duration::hours(h)))
            .collect(),
        ..Default::default()
    };
    let (r, mock) = run(
        &cfg_on(),
        &probes(vec![drain("TASK-1")]),
        &mut state,
        &ctx(),
    );
    assert!(!guard(&r, "wave-cap").pass);
    assert!(mock.spawns.is_empty());
}

// ---------------------------------------------------------------------------
// Off by default (A1), dry run, quiet ticks, status line
// ---------------------------------------------------------------------------

#[test]
fn shift_disabled_by_default_launches_nothing() {
    let cfg = build_config(None, None, REPO, "L");
    assert!(!cfg.enabled);
    let mut state = ShiftState::default();
    let (r, mock) = run(&cfg, &probes(vec![drain("TASK-1")]), &mut state, &ctx());
    assert!(r.launched.is_none());
    assert!(
        mock.calls.is_empty(),
        "no reap, tag, spawn, save or event: {:?}",
        mock.calls
    );
    // Another repo's switch does not enable this one.
    let other: toml::Value = toml::from_str("[repo.\"/elsewhere\"]\nenabled = true\n").unwrap();
    assert!(!build_config(None, Some(&other), REPO, "L").enabled);
}

#[test]
fn shift_committed_config_enable_alone_launches_nothing() {
    let cfg = cfg_with(Some("[shift]\nenabled = true\nwave_size = 4\n"), None);
    assert!(!cfg.enabled);
    assert!(cfg.committed_enable_ignored);
    assert_eq!(cfg.wave_size, 4, "committed tunables still apply");
    let (r, mock) = run(
        &cfg,
        &probes(vec![drain("TASK-1")]),
        &mut ShiftState::default(),
        &ctx(),
    );
    assert!(r.launched.is_none() && mock.calls.is_empty());
    assert!(guard(&r, "enabled").detail.contains("ignored"));
    // The local layer wins, and is the only place the switch is read.
    let local_off = cfg_with(
        Some("[shift]\nenabled = true\n"),
        Some(&format!("[repo.\"{REPO}\"]\nenabled = false\n")),
    );
    assert!(!local_off.enabled);
}

#[test]
fn shift_dry_run_on_fixture_queue_prints_argv_specs_guards_mail_and_writes_nothing() {
    use aida_core::DatabaseBackend;
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    let backend =
        aida_core::CachedGitBackend::open(&store, &tmp.path().join(".aida").join("cache.db"))
            .unwrap();
    let add = |spec: &str, mode: Option<ExecutionMode>, role: Option<&str>| {
        let mut r = aida_core::Requirement::new(spec.to_string(), "d".to_string());
        r.spec_id = Some(spec.to_string());
        r.status = RequirementStatus::Approved;
        r.execution_mode = mode;
        let r = backend.add_requirement(r).unwrap();
        backend
            .queue_add(aida_core::QueueEntry {
                user_id: "joe".into(),
                requirement_id: r.id,
                position: 0,
                added_by: "joe".into(),
                note: None,
                added_at: Utc::now(),
                for_role: role.map(str::to_string),
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })
            .unwrap();
    };
    add("TASK-1", Some(ExecutionMode::Drain), Some("implementer"));
    add("TASK-2", Some(ExecutionMode::Drive), Some("implementer"));
    add("TASK-3", Some(ExecutionMode::Drain), Some("reviewer"));
    let candidates = gather_candidates(tmp.path(), &backend, "joe");
    let specs: Vec<&str> = candidates.iter().map(|c| c.spec.as_str()).collect();
    assert!(
        specs.contains(&"TASK-1") && specs.contains(&"TASK-2"),
        "{specs:?}"
    );
    assert!(
        !specs.contains(&"TASK-3"),
        "a reviewer-routed entry is not the wave's view"
    );

    let mut c = ctx();
    c.dry_run = true;
    let mut state = ShiftState::default();
    let before = state.clone();
    let (r, mock) = run(&cfg_on(), &probes(candidates), &mut state, &c);
    assert!(
        mock.calls.is_empty(),
        "a dry run writes nothing: {:?}",
        mock.calls
    );
    assert_eq!(state, before);
    assert_eq!(r.specs, vec!["TASK-1"]);
    assert!(r
        .argv
        .windows(2)
        .any(|w| w[0] == "--batch" && w[1] == "shift-20260924-2210"));
    let text = render_report(&r);
    for needle in [
        "dry run",
        "enabled: yes",
        "user joe · role implementer",
        "command: aida queue work --batch shift-20260924-2210",
        "pass enabled",
        "skip TASK-2: execution mode drive",
        "mail latency:",
        "re-drive: off",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    // A dry run on a disabled clone still previews, naming the enable source.
    let off = build_config(None, None, REPO, "L");
    let (r_off, mock_off) = run(
        &off,
        &probes(vec![drain("TASK-1")]),
        &mut ShiftState::default(),
        &c,
    );
    assert!(mock_off.calls.is_empty());
    assert!(render_report(&r_off).contains("enabled: no — default (off)"));
}

#[test]
fn shift_auto_tag_replaces_prior_shift_tag() {
    use aida_core::DatabaseBackend;
    let mut tags: HashSet<String> = ["batch:shift-20260101-0000", "batch:followups", "x"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(retag(&mut tags, "shift-20260924-2210"));
    assert!(tags.contains("batch:shift-20260924-2210"));
    assert!(!tags.contains("batch:shift-20260101-0000"));
    assert!(tags.contains("batch:followups") && tags.contains("x"));
    assert!(!retag(&mut tags, "shift-20260924-2210"), "idempotent");

    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let backend = aida_core::CachedGitBackend::open(&store, &tmp.path().join("cache.db")).unwrap();
    let mut r = aida_core::Requirement::new("TASK-1".into(), "d".into());
    r.spec_id = Some("TASK-1".into());
    r.tags.insert("batch:shift-20260101-0000".into());
    backend.add_requirement(r).unwrap();
    tag_specs(&backend, "shift-20260924-2210", &["TASK-1".into()]).unwrap();
    let back = backend
        .get_requirement_by_spec_id("TASK-1")
        .unwrap()
        .unwrap();
    assert!(back.tags.contains("batch:shift-20260924-2210"));
    assert!(!back.tags.contains("batch:shift-20260101-0000"));
}

#[test]
fn shift_quiet_tick_emits_no_event() {
    let p = probes(vec![]);
    let mut state = ShiftState::default();
    let (_, first) = run(&cfg_on(), &p, &mut state, &ctx());
    assert_eq!(first.events.len(), 1, "the first hold is a verdict change");
    let (r, second) = run(&cfg_on(), &p, &mut state, &ctx());
    assert!(!r.event_emitted);
    assert!(second.events.is_empty(), "{:?}", second.events);
    assert!(
        second.calls.contains(&"save".to_string()),
        "last check time is still recorded"
    );
}

#[test]
fn status_line_silent_when_shift_disabled() {
    let off = build_config(None, None, REPO, "L");
    assert_eq!(
        status_line(&off, &ShiftState::default(), false, now()),
        None
    );
    let on = cfg_on();
    assert_eq!(
        status_line(&on, &ShiftState::default(), false, now()).unwrap(),
        "night shift: on · no check yet · idle"
    );
    let state = ShiftState {
        last_tick_at: Some(now() - Duration::minutes(4)),
        waves: vec![launched_wave(
            now() - Duration::minutes(30),
            &["A", "B", "C"],
        )],
        ..Default::default()
    };
    assert_eq!(
        status_line(&on, &state, true, now()).unwrap(),
        "night shift: on · last check 4m ago · working 3 items"
    );
    let holding = ShiftState {
        last_tick_at: Some(now() - Duration::minutes(4)),
        last_refused: vec!["token-budget".into()],
        ..Default::default()
    };
    assert!(status_line(&on, &holding, false, now())
        .unwrap()
        .ends_with("holding: token-budget"));
}

#[test]
fn shift_enable_writes_only_the_local_layer_and_registers_the_job() {
    let tmp = tempfile::tempdir().unwrap();
    let layer = tmp.path().join("home").join(".aida").join(LOCAL_LAYER_FILE);
    write_local_enabled(&layer, REPO, true).unwrap();
    let v = read_toml(&layer).unwrap();
    assert!(build_config(None, Some(&v), REPO, "L").enabled);
    write_local_enabled(&layer, REPO, false).unwrap();
    assert!(!build_config(None, read_toml(&layer).as_ref(), REPO, "L").enabled);

    let config = tmp.path().join("config.toml");
    std::fs::write(&config, "# keep me\n[schedule]\nmin_gap = \"60s\"\n").unwrap();
    let pairs = [
        ("name", toml_edit::Value::from(TICK_JOB_NAME)),
        ("command", toml_edit::Value::from(TICK_COMMAND)),
        ("every", toml_edit::Value::from(TICK_JOB_EVERY)),
        ("enabled", toml_edit::Value::from(true)),
    ];
    assert!(crate::config_edit::ensure_array_table_entry(
        &config,
        "schedule",
        "jobs",
        "command",
        TICK_COMMAND,
        &pairs
    )
    .unwrap());
    assert!(!crate::config_edit::ensure_array_table_entry(
        &config,
        "schedule",
        "jobs",
        "command",
        TICK_COMMAND,
        &pairs
    )
    .unwrap());
    let body = std::fs::read_to_string(&config).unwrap();
    assert!(
        body.contains("# keep me") && body.contains("command = \"shift tick\""),
        "{body}"
    );
    assert!(
        !body.contains("[shift]"),
        "the switch never lands in the project config"
    );
}

// ---------------------------------------------------------------------------
// Review round 1: operator gate, launch-path vendor, deadline, pid window
// ---------------------------------------------------------------------------

fn stopped_state() -> ShiftState {
    ShiftState {
        breaker: Some(Breaker {
            tripped_at: now(),
            reason: "2 consecutive shift waves made no progress".into(),
            queue_fingerprint: vec![],
        }),
        consecutive_zero_progress: 2,
        ..Default::default()
    }
}

/// `aida shift enable` arms unattended launches: it refuses without a human
/// at an interactive terminal, refuses in agent mode, and writes nothing
/// unless the human answers yes. Never touches the real `~/.aida`.
#[test]
fn shift_enable_requires_a_human_at_a_tty_and_a_yes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let layer = tmp.path().join("home").join(".aida").join(LOCAL_LAYER_FILE);
    let key = repo_key(&root);
    let enabled = |layer: &Path| build_config(None, read_toml(layer).as_ref(), &key, "L").enabled;

    for (tty, agent, label) in [
        (false, false, "not a TTY"),
        (true, true, "agent mode"),
        (false, true, "both"),
    ] {
        let mut asked = false;
        let mut confirm = |_: &str| {
            asked = true;
            Ok(true)
        };
        let mut op = Operator {
            stdin_tty: tty,
            agent_mode: agent,
            confirm: &mut confirm,
        };
        let err = enable_command(&root, &layer, &mut op).unwrap_err();
        assert!(
            format!("{err}").contains("interactive terminal"),
            "{label}: {err}"
        );
        assert!(!asked, "{label}: refused before prompting");
        assert!(!layer.exists(), "{label}: nothing written");
        assert!(!root.join(".aida").join("config.toml").exists(), "{label}");
    }

    let mut no = |_: &str| Ok(false);
    let mut op = Operator {
        stdin_tty: true,
        agent_mode: false,
        confirm: &mut no,
    };
    enable_command(&root, &layer, &mut op).unwrap();
    assert!(!layer.exists(), "declined: nothing written");

    let mut yes = |q: &str| {
        assert!(q.contains("[y/N]"), "{q}");
        Ok(true)
    };
    let mut op = Operator {
        stdin_tty: true,
        agent_mode: false,
        confirm: &mut yes,
    };
    enable_command(&root, &layer, &mut op).unwrap();
    assert!(enabled(&layer));

    // Disabling is always safe: no gate, no prompt.
    disable_command(&root, &layer).unwrap();
    assert!(!enabled(&layer));
}

/// `aida shift resume` clears the no-progress breaker: same floor.
#[test]
fn shift_resume_requires_a_human_at_a_tty_and_a_yes() {
    let tmp = tempfile::tempdir().unwrap();
    let path = state_path(tmp.path());
    save_state_to(&path, &stopped_state()).unwrap();
    for (tty, agent) in [(false, false), (true, true)] {
        let mut confirm = |_: &str| Ok(true);
        let mut op = Operator {
            stdin_tty: tty,
            agent_mode: agent,
            confirm: &mut confirm,
        };
        assert!(resume_command(tmp.path(), &mut op).is_err());
        assert!(
            load_state(&path).unwrap().breaker.is_some(),
            "refused: still stopped"
        );
    }
    let mut no = |_: &str| Ok(false);
    let mut op = Operator {
        stdin_tty: true,
        agent_mode: false,
        confirm: &mut no,
    };
    resume_command(tmp.path(), &mut op).unwrap();
    assert!(
        load_state(&path).unwrap().breaker.is_some(),
        "declined: still stopped"
    );
    let mut yes = |_: &str| Ok(true);
    let mut op = Operator {
        stdin_tty: true,
        agent_mode: false,
        confirm: &mut yes,
    };
    resume_command(tmp.path(), &mut op).unwrap();
    assert!(load_state(&path).unwrap().breaker.is_none());
    assert_eq!(operator_gate_refusal("resume", true, false), None);
}

/// A9: the probe resolves the vendor the way `queue work` launches. With
/// `[agents] enabled` listing only a non-claude profile and no explicit
/// vendor, the wave would spend on that profile, so the claude-only
/// watchdog evidence does not cover it and the guard refuses.
#[test]
fn shift_budget_guard_uses_the_launch_path_vendor() {
    let _env = crate::test_env::EnvVarGuard::unset("AIDA_HEADLESS_VENDOR");
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida").join("agents.toml"),
        "[agents]\nenabled = [\"codex\"]\n",
    )
    .unwrap();
    let (vendor, err) = resolve_wave_vendor(tmp.path());
    assert!(
        vendor != "claude" || err.is_some(),
        "the launch resolves to the only enabled profile (or refuses): {vendor} {err:?}"
    );
    let mut p = probes(vec![drain("TASK-1")]);
    p.vendor = vendor;
    p.vendor_error = err;
    let (r, mock) = run(&cfg_on(), &p, &mut ShiftState::default(), &ctx());
    assert!(!guard(&r, "budget-evidence").pass);
    assert!(mock.spawns.is_empty());

    // A resolution error alone refuses, even for a covered vendor name.
    let mut p = probes(vec![drain("TASK-1")]);
    p.vendor_error = Some("no agent launch profiles are enabled".into());
    let (r, _) = run(&cfg_on(), &p, &mut ShiftState::default(), &ctx());
    let g = guard(&r, "budget-evidence");
    assert!(
        !g.pass && g.detail.contains("cannot resolve"),
        "{}",
        g.detail
    );
}

/// A4: the clock is re-read after the reap and again before the spawn. A
/// slow reap that eats the reserve stops the launch; the tick still exits
/// cleanly.
#[test]
fn shift_deadline_rechecked_after_reap_before_spawn() {
    struct SlowReap(Mock);
    impl ShiftExec for SlowReap {
        fn reap(&mut self) -> usize {
            std::thread::sleep(StdDuration::from_millis(300));
            self.0.reap()
        }
        fn tag_batch(&mut self, b: &str, s: &[String]) -> Result<()> {
            self.0.tag_batch(b, s)
        }
        fn spawn_wave(&mut self, a: &[String], l: &Path) -> Result<(u32, Option<String>)> {
            self.0.spawn_wave(a, l)
        }
        fn save_state(&mut self, s: &ShiftState) -> Result<()> {
            self.0.save_state(s)
        }
        fn emit(&mut self, k: EventKind) {
            self.0.emit(k)
        }
    }
    let c = TickCtx::from_clock(
        now(),
        false,
        Instant::now(),
        LAUNCH_RESERVE + StdDuration::from_millis(150),
        PathBuf::from("/nonexistent/x.log"),
    );
    assert!(c.launch_allowed, "launch looked possible before the reap");
    let mut exec = SlowReap(Mock::default());
    let r = tick_core(
        &cfg_on(),
        &probes(vec![drain("TASK-1")]),
        &mut ShiftState::default(),
        &c,
        &mut exec,
    )
    .unwrap();
    assert!(exec.0.calls.contains(&"reap".to_string()));
    assert!(exec.0.spawns.is_empty() && r.launched.is_none());
    assert!(!guard(&r, "deadline").pass);
}

/// A4: a tick killed after the spawn but before the pid was recorded leaves
/// an intent with no pid while the wave runs. The wave holds the drain
/// lock, so the next tick's lock guard stops a second launch of the same
/// batch; once the wave is gone, members it took are no longer eligible.
#[test]
fn shift_killed_between_spawn_and_pid_record_is_held_by_the_drain_lock() {
    let earlier = now() - Duration::minutes(15);
    let mut state = ShiftState {
        waves: vec![WaveRecord {
            batch: shift_batch_name(earlier),
            specs: vec!["TASK-1".into(), "TASK-2".into()],
            at: earlier,
            argv: Vec::new(),
            pid: None,
            pid_start: None,
            log: None,
            outcome: None,
        }],
        ..Default::default()
    };
    let mut p = probes(vec![drain("TASK-1"), drain("TASK-2")]);
    p.lock = LockView::Running(9999);
    let (r, mock) = run(&cfg_on(), &p, &mut state, &ctx());
    assert!(r.reused_batch, "it recognises the interrupted batch");
    assert!(!guard(&r, "lock-free").pass);
    assert!(r.launched.is_none() && mock.spawns.is_empty() && mock.tags.is_empty());

    // The wave finished: it took TASK-1; TASK-2 is still queued and eligible.
    let mut after = probes(
        vec![RequirementStatus::Completed, RequirementStatus::Approved]
            .into_iter()
            .zip(["TASK-1", "TASK-2"])
            .map(|(st, id)| Candidate {
                status: st,
                ..drain(id)
            })
            .collect(),
    );
    after.lock = LockView::Free;
    let (r2, mock2) = run(&cfg_on(), &after, &mut state, &ctx());
    assert_eq!(r2.launched.unwrap().specs, vec!["TASK-2"]);
    assert_eq!(mock2.spawns.len(), 1);
}

/// A4: the last clock read sits between tagging and spawning. A slow store
/// write that eats the reserve leaves the intent (no pid) for the next tick
/// to reuse, and spawns nothing.
#[test]
fn shift_deadline_rechecked_between_tag_and_spawn() {
    struct SlowTag(Mock);
    impl ShiftExec for SlowTag {
        fn reap(&mut self) -> usize {
            self.0.reap()
        }
        fn tag_batch(&mut self, b: &str, s: &[String]) -> Result<()> {
            std::thread::sleep(StdDuration::from_millis(400));
            self.0.tag_batch(b, s)
        }
        fn spawn_wave(&mut self, a: &[String], l: &Path) -> Result<(u32, Option<String>)> {
            self.0.spawn_wave(a, l)
        }
        fn save_state(&mut self, s: &ShiftState) -> Result<()> {
            self.0.save_state(s)
        }
        fn emit(&mut self, k: EventKind) {
            self.0.emit(k)
        }
    }
    let c = TickCtx::from_clock(
        now(),
        false,
        Instant::now(),
        LAUNCH_RESERVE + StdDuration::from_millis(200),
        PathBuf::from("/nonexistent/x.log"),
    );
    let mut exec = SlowTag(Mock::default());
    let mut state = ShiftState::default();
    let r = tick_core(
        &cfg_on(),
        &probes(vec![drain("TASK-1")]),
        &mut state,
        &c,
        &mut exec,
    )
    .unwrap();
    assert!(exec.0.calls.contains(&"tag".to_string()));
    assert!(exec.0.spawns.is_empty() && r.launched.is_none());
    assert!(!guard(&r, "deadline").pass);
    let intent = state.waves.last().expect("intent kept for reuse");
    assert_eq!(intent.pid, None);
    assert_eq!(state.waves_in_day(now()), 0);
}

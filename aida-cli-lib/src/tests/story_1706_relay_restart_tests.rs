//! Tests for STORY-1706: the drive tick restarts a crashed relay from its
//! human launch record, under every condition of the advisor's ruling, and
//! fails closed (routes to the advisor as before) otherwise.
//
// trace:STORY-1706 | ai:claude

use super::*;
use crate::relay_launch::{self as rl, RestartLine};
use std::cell::{Cell, RefCell};
use std::os::unix::fs::PermissionsExt;

/// Each fixture's run id is unique: the orphan-session check scans every
/// process for `AIDA_RELAY_RUN=<run>`, and tests run in parallel.
fn fresh_run() -> String {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("20261022-040000-impl-{}-{n}", std::process::id())
}

fn ok_human() -> Result<()> {
    Ok(())
}

fn t0() -> DateTime<Utc> {
    "2026-10-22T12:00:00Z".parse().unwrap()
}

fn no_spec(_: &str) -> Option<String> {
    None
}

fn task_77(id: &str) -> Option<bool> {
    (id == "TASK-77").then_some(false)
}

/// A pid that is certainly dead: a reaped child's.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

fn write_0600(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[derive(Default)]
struct Fake {
    due: Vec<Planned>,
    notices: Vec<(String, String)>,
    rows: Vec<ActionRow>,
    restarts: Vec<RestartPlan>,
    fail_restart: bool,
}

impl DriveEffects for Fake {
    fn emit_due(&mut self, _: &str, item: &Planned) -> Result<()> {
        self.due.push(item.clone());
        Ok(())
    }
    fn notify_operator(&mut self, title: &str, body: &str) -> Result<String> {
        self.notices.push((title.into(), body.into()));
        Ok("sent".into())
    }
    fn reap_dead_leases(&mut self) -> Vec<ReapRow> {
        Vec::new()
    }
    fn ledger(&mut self, _: &str, row: &ActionRow) {
        self.rows.push(row.clone());
    }
    fn restart_relay(&mut self, plan: &RestartPlan) -> Result<u32> {
        if self.fail_restart {
            anyhow::bail!("exec format error");
        }
        self.restarts.push(plan.clone());
        Ok(4242)
    }
}

impl Fake {
    fn restart_rows(&self) -> Vec<&ActionRow> {
        self.rows
            .iter()
            .filter(|r| r.action == "relay-restart")
            .collect()
    }
}

/// A crashed implementer run on disk: relay.status says running with a dead
/// pid and no lock file, a plain handoff, an executable relay.sh, and a
/// human launch record.
struct Fixture {
    run: String,
    root: tempfile::TempDir,
    mandates: tempfile::TempDir,
    run_dir: PathBuf,
    record: BTreeMap<&'static str, String>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mandates = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let run = fresh_run();
        let run_dir = root_path.join(".aida/relay").join(&run);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::create_dir_all(root_path.join("bin")).unwrap();
        let relay_sh = root_path.join("bin/relay.sh");
        std::fs::write(&relay_sh, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&relay_sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(run_dir.join("handoff.md"), "carry on with TASK-77\n").unwrap();
        let root_s = root_path.display().to_string();
        let record = BTreeMap::from([
            ("v", "1".to_string()),
            ("run", run.clone()),
            ("launched_at", "2026-10-22T04:00:00-0700".to_string()),
            ("launch_cwd", root_s.clone()),
            ("project_root", root_s),
            ("relay_sh", relay_sh.display().to_string()),
            ("agent", "claude".to_string()),
            ("session_role", "implementer".to_string()),
            ("force_seat", "0".to_string()),
            ("max_sessions", "10".to_string()),
            ("start_session", "1".to_string()),
            ("profile_ref", String::new()),
            ("profile_file", String::new()),
            ("profile_sha256", String::new()),
            ("principal", "joe".to_string()),
            ("launched_from_tty", "1".to_string()),
        ]);
        let fx = Self {
            run,
            root,
            mandates,
            run_dir,
            record,
        };
        fx.write_status(&[]);
        fx.write_record();
        fx
    }

    /// The record's resume argv.
    fn args(&self) -> Vec<String> {
        ["--run", &self.run, "--use-existing", "--non-interactive"]
            .map(String::from)
            .to_vec()
    }

    fn root(&self) -> PathBuf {
        self.root.path().canonicalize().unwrap()
    }

    fn write_status(&self, extra: &[(&str, &str)]) {
        let mut map = BTreeMap::from([
            ("state".to_string(), "running".to_string()),
            ("pid".to_string(), dead_pid().to_string()),
            ("run".to_string(), self.run.clone()),
            ("session".to_string(), "1".to_string()),
            ("ticket".to_string(), "TASK-77".to_string()),
            (
                "handoff".to_string(),
                self.run_dir.join("handoff.md").display().to_string(),
            ),
            ("exit".to_string(), String::new()),
            (
                "updated".to_string(),
                "2026-10-22T04:30:00-0700".to_string(),
            ),
            ("reason".to_string(), String::new()),
        ]);
        for (k, v) in extra {
            map.insert(k.to_string(), v.to_string());
        }
        let body: String = map.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        std::fs::write(self.run_dir.join("relay.status"), body).unwrap();
    }

    fn write_record(&self) {
        let mut body: String = self
            .record
            .iter()
            .map(|(k, v)| format!("{k}={v}\n"))
            .collect();
        for a in self.args() {
            body.push_str(&format!("arg={a}\n"));
        }
        write_0600(&self.run_dir.join(rl::RECORD_FILE), &body);
    }

    fn set(&mut self, key: &'static str, value: &str) {
        self.record.insert(key, value.to_string());
        self.write_record();
    }

    fn sha(&self) -> String {
        rl::load(&self.run_dir).unwrap().sha256
    }

    fn lane(&self) -> Lane {
        crate::fleet_lanes::probe_lane(self.run_dir.clone()).unwrap()
    }

    fn mandate(&self) -> Mandate {
        crate::mandate::start_with(
            self.mandates.path(),
            &ok_human,
            "joe",
            Duration::hours(8),
            Some("fire".into()),
            true,
            true,
            t0(),
        )
        .unwrap()
    }

    fn verdict(&self, m: &Mandate, now: DateTime<Utc>) -> Option<RestartVerdict> {
        restart_verdict(
            &self.lane(),
            m,
            &self.root(),
            &Ok(HumanGates::default()),
            &no_spec,
            &|id| task_77(id).is_some(),
            now,
        )
    }

    fn inputs<'a>(
        &'a self,
        root: &'a Path,
        clock: &'a dyn Fn() -> DateTime<Utc>,
    ) -> TickInputs<'a> {
        TickInputs {
            project_root: root,
            mandate_dir: self.mandates.path(),
            principal: "joe",
            clock,
            gates: Ok(HumanGates::default()),
            lanes: vec![self.lane()],
            held_prs: Vec::new(),
            advisor_live: true,
            inherited_session_env: false,
            spec_always_human: &no_spec,
            spec_signoff: &task_77,
            store_readable: true,
            unowned_specs: Vec::new(),
        }
    }

    fn lines(&self) -> Vec<RestartLine> {
        rl::restarts(&self.run_dir, &self.sha()).unwrap()
    }

    fn add_line(&self, at: DateTime<Utc>, max: u32, used: u32) {
        let n = self.lines().len() as u32 + 1;
        rl::append_restart(
            &self.run_dir,
            &RestartLine {
                generation: n,
                at,
                launch: self.sha(),
                mandate: "m".into(),
                max_sessions: max,
                used_before: used,
            },
        )
        .unwrap();
    }
}

fn now() -> DateTime<Utc> {
    t0() + Duration::hours(1)
}

fn expect_no(v: Option<RestartVerdict>, needle: &str) {
    match v {
        Some(RestartVerdict::No(why)) => assert!(why.contains(needle), "{why}"),
        other => panic!("want No({needle}), got {other:?}"),
    }
}

// ── The crash ending only ───────────────────────────────────────────────────

#[test]
fn only_the_trap_never_ran_ending_is_a_restart_candidate() {
    let fx = Fixture::new();
    assert!(fx.lane().evidence.trap_never_ran());
    // Each of these is an ending the trap wrote, including an unknown exit
    // code (Crashed, but not the crash ending).
    for status in [
        vec![
            ("state", "exited"),
            ("exit", "143"),
            ("reason", "signal:TERM"),
        ],
        vec![
            ("state", "exited"),
            ("exit", "0"),
            ("reason", "session-cap"),
        ],
        vec![
            ("state", "exited"),
            ("exit", "1"),
            ("reason", "setup-error"),
        ],
        vec![("state", "exited"), ("exit", "42")],
        vec![("state", "running"), ("reason", "signal:INT")],
    ] {
        fx.write_status(&status);
        let lane = fx.lane();
        assert!(!lane.evidence.trap_never_ran(), "{status:?}");
        assert_eq!(fx.verdict(&fx.mandate_once(), now()), None, "{status:?}");
    }
}

impl Fixture {
    /// The active mandate, minting it on first use.
    fn mandate_once(&self) -> Mandate {
        let (valid, _) = crate::mandate::load_valid(self.mandates.path(), "joe", now());
        match crate::mandate::state_at(&valid, now()) {
            MandateState::Active(m) => m,
            _ => self.mandate(),
        }
    }
}

#[test]
fn a_held_lock_or_an_unknown_owner_is_not_a_crash() {
    let fx = Fixture::new();
    let mut lane = fx.lane();
    lane.evidence.lock_held = None;
    assert!(!lane.evidence.trap_never_ran());
    lane.evidence.lock_held = Some(false);
    lane.evidence.owner_alive = None;
    assert!(!lane.evidence.trap_never_ran());
}

// ── An eligible restart ─────────────────────────────────────────────────────

#[test]
fn an_eligible_crash_restarts_from_the_record_argv_only() {
    let fx = Fixture::new();
    fx.mandate();
    let root = fx.root();
    let clock = || now();
    let inputs = fx.inputs(&root, &clock);
    let mut fake = Fake::default();
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert_eq!(fake.restarts.len(), 1, "{:?}", report.rows);
    let plan = &fake.restarts[0];
    let mut want = fx.args();
    want.extend(["--max-sessions".into(), "9".into()]);
    assert_eq!(
        plan.argv, want,
        "C1: the record's arg= lines plus --max-sessions"
    );
    assert_eq!(plan.cwd, root);
    assert_eq!(plan.relay_sh, root.join("bin/relay.sh"));
    assert_eq!(plan.launch_sha, fx.sha());
    assert_eq!(plan.generation, 1);
    let rows = fake.restart_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].outcome, Outcome::Done, "{:?}", rows[0]);
    // The lineage line is on disk (C2) and the lane is not also routed.
    let lines = fx.lines();
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].max_sessions, lines[0].used_before), (9, 1));
    assert!(fake.due.is_empty(), "{:?}", fake.due);
}

#[test]
fn the_record_argv_ignores_provenance_lines_and_the_environment() {
    let fx = Fixture::new();
    let m = fx.mandate();
    let mut body = std::fs::read_to_string(fx.run_dir.join(rl::RECORD_FILE)).unwrap();
    body.push_str("orig_arg=--label\norig_arg=impl\norig_arg=do it\n");
    write_0600(&fx.run_dir.join(rl::RECORD_FILE), &body);
    let Some(RestartVerdict::Restart(plan)) = fx.verdict(&m, now()) else {
        panic!("not eligible");
    };
    assert_eq!(plan.argv[..4], fx.args()[..]);
    assert_eq!(plan.argv.len(), 6);
}

// ── Every refusal condition ─────────────────────────────────────────────────

#[test]
fn refusals_each_condition() {
    type Mutate = fn(&mut Fixture);
    let cases: Vec<(&str, Mutate, &str)> = vec![
        (
            "principal",
            |f| f.set("principal", "eve"),
            "not the mandate's principal",
        ),
        ("tty", |f| f.set("launched_from_tty", "0"), "at a terminal"),
        (
            "role",
            |f| f.set("session_role", "advisor"),
            "advisor or --force-seat",
        ),
        (
            "force",
            |f| f.set("force_seat", "1"),
            "advisor or --force-seat",
        ),
        ("version", |f| f.set("v", "2"), "unknown record version"),
        ("run", |f| f.set("run", "other-run"), "names run other-run"),
        (
            "root",
            |f| f.set("project_root", "/"),
            "another project root",
        ),
        (
            "relay_sh",
            |f| {
                let p = f.root().join("bin/relay.sh");
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
            },
            "not an executable file",
        ),
        (
            "missing",
            |f| std::fs::remove_file(f.run_dir.join(rl::RECORD_FILE)).unwrap(),
            "no usable launch record",
        ),
        (
            "mode",
            |f| {
                let p = f.run_dir.join(rl::RECORD_FILE);
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
            },
            "not 600",
        ),
        (
            "symlink",
            |f| {
                let p = f.run_dir.join(rl::RECORD_FILE);
                let real = f.run_dir.join("elsewhere");
                std::fs::rename(&p, &real).unwrap();
                std::os::unix::fs::symlink(&real, &p).unwrap();
            },
            "no usable launch record",
        ),
        (
            "norestart",
            |f| std::fs::write(f.run_dir.join(rl::NORESTART_FILE), "by=joe\n").unwrap(),
            "--no-restart",
        ),
        (
            "complete",
            |f| std::fs::write(f.run_dir.join("handoff.md"), "TASK_COMPLETE\n").unwrap(),
            "complete or blocked",
        ),
        (
            "profile changed",
            |f| {
                let p = f.root().join("impl.toml");
                std::fs::write(&p, "role = \"implementer\"\n").unwrap();
                f.set("profile_file", &p.display().to_string());
                f.set("profile_sha256", "00");
            },
            "profile",
        ),
        (
            "profile gone",
            |f| {
                let p = f.root().join("gone.toml");
                f.set("profile_file", &p.display().to_string());
                f.set("profile_sha256", &rl::sha256_hex(b"x"));
            },
            "profile",
        ),
        (
            "unknown spec",
            |f| f.write_status(&[("ticket", "slot-queue/ticket-1")]),
            "could not be determined",
        ),
        (
            "no sessions left",
            |f| f.write_status(&[("session", "10")]),
            "no sessions left",
        ),
    ];
    for (name, mutate, needle) in cases {
        let mut fx = Fixture::new();
        let m = fx.mandate();
        mutate(&mut fx);
        expect_no(fx.verdict(&m, now()), needle);
        // The tick then routes the lane as before (the advisor, or the
        // operator when gated), saying why it was not restarted.
        let root = fx.root();
        let clock = || now();
        let inputs = fx.inputs(&root, &clock);
        let mut fake = Fake::default();
        run_tick(&inputs, &TickHooks::default(), &mut fake);
        assert!(fake.restarts.is_empty(), "{name}");
        let said = fake
            .due
            .iter()
            .any(|d| d.detail.contains("not auto-restarted"))
            || fake
                .notices
                .iter()
                .any(|(_, b)| b.contains("not auto-restarted"));
        assert!(said, "{name}: {:?} {:?}", fake.due, fake.notices);
    }
}

/// Advisor ruling 12:55: the record's `spec=` (from `--spec` or the
/// profile's `spec` key) names the run's spec when relay.status does not.
/// Absent, malformed or unknown to the store, it is still refused.
// trace:STORY-1706 | ai:claude
#[test]
fn the_record_spec_names_the_run_when_the_status_does_not() {
    let mut fx = Fixture::new();
    let m = fx.mandate();
    fx.write_status(&[("ticket", "slot-queue/ticket-1")]);
    fx.set("spec", "TASK-77");
    match fx.verdict(&m, now()) {
        Some(RestartVerdict::Restart(_)) => {}
        other => panic!("want a restart from the record's spec, got {other:?}"),
    }
    for bad in [
        "",
        "TASK-99",
        "task-77",
        "TASK-77x",
        " TASK-77",
        "TASK-77 TASK-78",
    ] {
        fx.set("spec", bad);
        expect_no(fx.verdict(&m, now()), "could not be determined");
    }
}

#[test]
fn an_advisor_seat_run_is_never_restarted() {
    let fx = Fixture::new();
    let m = fx.mandate();
    let advisor = fx.root().join(".aida/handoff/advisor/latest.md");
    std::fs::create_dir_all(advisor.parent().unwrap()).unwrap();
    std::fs::write(&advisor, "carry on\n").unwrap();
    fx.write_status(&[("handoff", &advisor.display().to_string())]);
    expect_no(fx.verdict(&m, now()), "advisor-seat");
}

#[test]
fn an_operator_only_spec_is_never_restarted() {
    let fx = Fixture::new();
    let m = fx.mandate();
    let human = |id: &str| (id == "TASK-77").then(|| "operator-only".to_string());
    let v = restart_verdict(
        &fx.lane(),
        &m,
        &fx.root(),
        &Ok(HumanGates::default()),
        &human,
        &|id| task_77(id).is_some(),
        now(),
    );
    expect_no(v, "TASK-77 is operator-only");
}

#[test]
fn a_relay_restart_human_gate_or_unreadable_gates_refuse() {
    let fx = Fixture::new();
    let m = fx.mandate();
    let gated = Ok(HumanGates {
        ops: BTreeSet::from([GateOp::RelayRestart]),
        max_actions_per_tick: None,
    });
    for gates in [gated, Err("origin/main unreadable".to_string())] {
        let v = restart_verdict(
            &fx.lane(),
            &m,
            &fx.root(),
            &gates,
            &no_spec,
            &|id| task_77(id).is_some(),
            now(),
        );
        expect_no(v, "gated");
    }
    let layer = crate::mandate_gates::parse_layer(
        "[mandate.human_gates]\nops = [\"relay-restart\"]\n",
        "test",
    );
    assert!(layer.is_ok(), "{layer:?}");
}

#[test]
fn a_drive_record_without_the_scope_does_not_cover_restarts() {
    let fx = Fixture::new();
    let mut m = fx.mandate();
    assert!(
        m.covers(Scope::RelayRestart),
        "--drive writes relay-restart"
    );
    m.scopes.remove(&Scope::RelayRestart);
    expect_no(fx.verdict(&m, now()), "does not cover relay-restart");
    // A record written before the scope existed loads without it.
    let old = serde_json::json!({
        "id": "6f7c3a52-1a0e-4c1e-9b51-2f1f3f0f9d10",
        "principal": "joe", "seat": "advisor", "scope": "ship-held",
        "issued_at": t0(), "expires_at": t0() + Duration::hours(8),
        "scopes": ["ship-held", "sketch-signoff", "dispatch"], "kind": "drive"
    });
    let old: Mandate = serde_json::from_value(old).unwrap();
    assert!(!old.covers(Scope::RelayRestart));
}

#[test]
fn less_than_thirty_minutes_left_refuses() {
    let fx = Fixture::new();
    let m = fx.mandate();
    let late = m.expires_at - Duration::minutes(29);
    expect_no(fx.verdict(&m, late), "30 minutes");
    let ok = m.expires_at - Duration::minutes(31);
    assert!(matches!(
        fx.verdict(&m, ok),
        Some(RestartVerdict::Restart(_))
    ));
}

#[test]
fn a_live_session_of_the_run_refuses_the_restart() {
    let fx = Fixture::new();
    let m = fx.mandate();
    assert!(matches!(
        fx.verdict(&m, now()),
        Some(RestartVerdict::Restart(_))
    ));
    let mut orphan = std::process::Command::new("sleep")
        .arg("30")
        .env("AIDA_RELAY_RUN", &fx.run)
        .spawn()
        .unwrap();
    let v = fx.verdict(&m, now());
    orphan.kill().unwrap();
    orphan.wait().unwrap();
    expect_no(v, "still running");
    assert!(matches!(
        fx.verdict(&m, now()),
        Some(RestartVerdict::Restart(_))
    ));
}

// ── Budget, backoff and the cap (C2, C3) ────────────────────────────────────

#[test]
fn restarts_back_off_one_two_four_ticks() {
    let fx = Fixture::new();
    let m = fx.mandate();
    let at = now();
    for (gap_ticks, generation) in [(1, 2u32), (2, 3)] {
        fx.add_line(at, 9, 1);
        let just_before = at + TICK_INTERVAL * gap_ticks - Duration::seconds(1);
        assert!(
            matches!(fx.verdict(&m, just_before), Some(RestartVerdict::Wait(_))),
            "restart {generation} waits {gap_ticks} tick(s)"
        );
        let Some(RestartVerdict::Restart(p)) = fx.verdict(&m, at + TICK_INTERVAL * gap_ticks)
        else {
            panic!("restart {generation} is due after {gap_ticks} tick(s)");
        };
        assert_eq!(p.generation, generation);
    }
    // The 4-tick step exists for a fourth attempt; the cap stops at three.
    assert_eq!(BACKOFF_TICKS, [1, 2, 4]);
}

#[test]
fn the_session_budget_spans_the_lineage() {
    let fx = Fixture::new();
    let m = fx.mandate();
    // The first restart passed 9 after one used session; this instance
    // crashed in its session 4 (sessions start at 1 on a resume).
    fx.add_line(now() - Duration::hours(1), 9, 1);
    fx.write_status(&[("session", "4")]);
    let Some(RestartVerdict::Restart(p)) = fx.verdict(&m, now()) else {
        panic!("eligible");
    };
    assert_eq!(p.max_sessions, 5, "10 - (1 + 4)");
    assert_eq!(p.used_before, 5);
    assert_eq!(p.argv.last().map(String::as_str), Some("5"));
}

#[test]
fn the_crash_loop_notice_is_sent_once_per_launch_over_two_ticks() {
    let fx = Fixture::new();
    fx.mandate();
    for _ in 0..rl::MAX_RESTARTS {
        fx.add_line(now() - Duration::hours(2), 9, 1);
    }
    let root = fx.root();
    let clock = || now();
    let mut fake = Fake::default();
    run_tick(&fx.inputs(&root, &clock), &TickHooks::default(), &mut fake);
    // A second crash of the same lineage, a tick later.
    fx.write_status(&[("updated", "2026-10-22T05:00:00-0700")]);
    let later = || now() + TICK_INTERVAL;
    run_tick(&fx.inputs(&root, &later), &TickHooks::default(), &mut fake);
    let loops: Vec<_> = fake
        .notices
        .iter()
        .filter(|(t, _)| t.contains("crash-looping"))
        .collect();
    assert_eq!(loops.len(), 1, "{:?}", fake.notices);
    assert!(fake.restarts.is_empty());
    assert!(fake
        .due
        .iter()
        .all(|d| d.detail.contains("not auto-restarted")));
    assert!(!fake.due.is_empty(), "the lane still routes to the advisor");
}

// ── Mid-tick changes and failures ───────────────────────────────────────────

#[test]
fn a_mandate_stopped_before_the_exec_refuses_without_a_spawn() {
    let fx = Fixture::new();
    fx.mandate();
    let root = fx.root();
    let clock = || now();
    let dir = fx.mandates.path().to_path_buf();
    // Side effect 0 is the lease reap, 1 the restart.
    let hook = |n: usize| {
        if n == 1 {
            crate::mandate::stop(&dir, &ok_human, "joe", now()).unwrap();
        }
    };
    let hooks = TickHooks {
        before_action: Some(&hook),
        ..Default::default()
    };
    let mut fake = Fake::default();
    run_tick(&fx.inputs(&root, &clock), &hooks, &mut fake);
    assert!(fake.restarts.is_empty());
    assert!(fx.lines().is_empty(), "no lineage line without an exec");
    let rows = fake.restart_rows();
    assert!(
        rows.iter().any(|r| r.outcome == Outcome::Refused
            && r.why.as_deref().is_some_and(|w| w.contains("stopped"))),
        "{rows:?}"
    );
}

#[test]
fn a_lane_that_relocks_before_the_exec_is_left_alone() {
    let fx = Fixture::new();
    fx.mandate();
    let root = fx.root();
    let clock = || now();
    let held: RefCell<Option<std::fs::File>> = RefCell::new(None);
    let lock_path = fx.run_dir.join("relay.lock");
    let hook = |n: usize| {
        if n == 1 {
            let f = std::fs::File::create(&lock_path).unwrap();
            fs2::FileExt::lock_exclusive(&f).unwrap();
            *held.borrow_mut() = Some(f);
        }
    };
    let hooks = TickHooks {
        before_action: Some(&hook),
        ..Default::default()
    };
    let mut fake = Fake::default();
    run_tick(&fx.inputs(&root, &clock), &hooks, &mut fake);
    assert!(fake.restarts.is_empty());
    assert!(fx.lines().is_empty());
    assert!(fake.restart_rows().iter().any(|r| r
        .why
        .as_deref()
        .is_some_and(|w| w.contains("changed before"))));
    assert!(fake.due.is_empty(), "a live relay again is not routed");
}

#[test]
fn an_unwritable_lineage_means_no_exec() {
    let fx = Fixture::new();
    fx.mandate();
    let root = fx.root();
    let clock = || now();
    let run_dir = fx.run_dir.clone();
    let hook = |n: usize| {
        if n == 1 {
            std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        }
    };
    let hooks = TickHooks {
        before_action: Some(&hook),
        ..Default::default()
    };
    let mut fake = Fake::default();
    run_tick(&fx.inputs(&root, &clock), &hooks, &mut fake);
    std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(fake.restarts.is_empty());
    assert!(fake.restart_rows().iter().any(|r| r
        .why
        .as_deref()
        .is_some_and(|w| w.contains("cannot record"))));
}

#[test]
fn a_failed_spawn_counts_toward_the_cap_and_backs_off() {
    let fx = Fixture::new();
    fx.mandate();
    let root = fx.root();
    let clock = || now();
    let mut fake = Fake {
        fail_restart: true,
        ..Default::default()
    };
    run_tick(&fx.inputs(&root, &clock), &TickHooks::default(), &mut fake);
    assert_eq!(fx.lines().len(), 1);
    assert!(fake
        .restart_rows()
        .iter()
        .any(|r| r.outcome == Outcome::Refused));
    assert!(fake.due.is_empty(), "the next tick retries; not routed");
    // Five minutes on: still backing off, and still not routed.
    let soon = || now() + Duration::minutes(5);
    let mut fake2 = Fake::default();
    run_tick(&fx.inputs(&root, &soon), &TickHooks::default(), &mut fake2);
    assert!(fake2.restarts.is_empty());
    assert!(fake2.due.is_empty());
}

#[test]
fn the_restart_respects_the_per_tick_budget() {
    let fx = Fixture::new();
    fx.mandate();
    let root = fx.root();
    let clock = || now();
    let mut inputs = fx.inputs(&root, &clock);
    inputs.gates = Ok(HumanGates {
        ops: BTreeSet::new(),
        max_actions_per_tick: Some(0),
    });
    let mut fake = Fake::default();
    run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert!(fake.restarts.is_empty());
    assert!(fx.lines().is_empty());
}

// ── The live spawn ──────────────────────────────────────────────────────────

#[test]
fn the_live_spawn_scrubs_the_environment_and_detaches() {
    let fx = Fixture::new();
    let root = fx.root();
    let out = root.join("spawned.txt");
    let stub = root.join("bin/relay.sh");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\n{{ echo \"pwd=$(pwd)\"; echo \"pgid=$(ps -o pgid= -p $$ | tr -d ' ') pid=$$\"; for a in \"$@\"; do echo \"arg=$a\"; done; env | grep -E '^(AIDA_|CLAUDE)' | sort; }} > {}.tmp && mv {}.tmp {}\n",
            out.display(),
            out.display(),
            out.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_SESSION_GRANT", "g-secret"),
        ("AIDA_SESSION_ROLE", "advisor"),
        ("AIDA_SESSION_PURPOSE", "drive"),
        ("AIDA_AGENT_NAME", "drain-worker"),
        ("AIDA_AGENT_TYPE", "claude"),
        ("AIDA_RELAY_RESTART", "stale"),
    ]);
    let plan = RestartPlan {
        lane: format!("relay:{}", fx.run),
        run_id: fx.run.clone(),
        run_dir: fx.run_dir.clone(),
        relay_sh: stub,
        cwd: root.clone(),
        argv: vec![
            "--run".into(),
            fx.run.clone(),
            "a b".into(),
            "--max-sessions".into(),
            "3".into(),
        ],
        launch_sha: "abc123".into(),
        generation: 1,
        max_sessions: 3,
        used_before: 1,
        crashed_head: "-".into(),
    };
    let pid = spawn_restart(&plan).unwrap();
    let done = Cell::new(false);
    for _ in 0..100 {
        if out.exists() {
            done.set(true);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(done.get(), "the stub never ran");
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains(&format!("pwd={}", root.display())), "{text}");
    assert!(
        text.contains(&format!("pgid={pid} pid={pid}")),
        "own process group: {text}"
    );
    let args: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("arg="))
        .collect();
    assert_eq!(
        args,
        ["--run", fx.run.as_str(), "a b", "--max-sessions", "3"]
    );
    for var in [
        "AIDA_SESSION_GRANT",
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_PURPOSE",
        "AIDA_AGENT_NAME",
        "AIDA_AGENT_TYPE",
    ] {
        assert!(!text.contains(&format!("{var}=")), "{var} leaked: {text}");
    }
    assert!(text.contains("AIDA_RELAY_RESTART=abc123"), "{text}");
    assert!(text.contains("AIDA_SCHEDULE_CHILD=1"), "{text}");
    assert!(fx.run_dir.join(rl::RESTART_LOG).exists());
}

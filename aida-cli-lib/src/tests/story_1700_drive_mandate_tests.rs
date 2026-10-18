//! Tests for STORY-1700: the drive mandate (ADR-77) — the record, the
//! display lines, the human gates, the drive tick (R2-R6) and its ledger.
//! T7.4-T7.14 of `docs/plans/2026-10-15-story-1700-drive-mandate.md`.
//
// trace:STORY-1700 | ai:claude

use super::*;
use crate::fleet_lanes::{LaneEvidence, SeatKind};
use std::cell::Cell;

fn ok_human() -> Result<()> {
    Ok(())
}

fn no_tty() -> Result<()> {
    anyhow::bail!("not a terminal")
}

fn t0() -> DateTime<Utc> {
    "2026-10-17T12:00:00Z".parse().unwrap()
}

/// A drive mandate for `principal` minted at `t0`, 8h.
fn drive_mandate(dir: &Path, principal: &str) -> Mandate {
    crate::mandate::start_with(
        dir,
        &ok_human,
        principal,
        Duration::hours(8),
        Some("fire".into()),
        true,
        true,
        t0(),
    )
    .unwrap()
}

fn lane(run: &str, class: LaneClass, run_dir: &Path, status: &[(&str, &str)]) -> Lane {
    let mut map: BTreeMap<String, String> = status
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    map.entry("updated".into())
        .or_insert_with(|| "2026-10-17T11:00:00Z".into());
    map.entry("handoff".into())
        .or_insert_with(|| format!("{}/sessions/h.md", run_dir.display()));
    Lane {
        run_dir: run_dir.to_path_buf(),
        run_id: run.into(),
        seat_kind: SeatKind::Implementer,
        class,
        evidence: LaneEvidence {
            status: map,
            lock_held: Some(false),
            owner_alive: Some(false),
            acknowledged: false,
        },
    }
}

#[derive(Default)]
struct Fake {
    due: Vec<(String, Planned)>,
    notices: Vec<(String, String)>,
    rows: Vec<(String, ActionRow)>,
    reaped: usize,
    fail_due_for: Option<String>,
}

impl DriveEffects for Fake {
    fn emit_due(&mut self, mandate_id: &str, item: &Planned) -> Result<()> {
        if self.fail_due_for.as_deref() == Some(item.lane.as_str()) {
            anyhow::bail!("events.jsonl is read-only");
        }
        self.due.push((mandate_id.into(), item.clone()));
        Ok(())
    }
    fn notify_operator(&mut self, title: &str, body: &str) -> Result<String> {
        self.notices.push((title.into(), body.into()));
        Ok("sent".into())
    }
    fn reap_dead_leases(&mut self) -> Result<usize> {
        Ok(std::mem::take(&mut self.reaped))
    }
    fn ledger(&mut self, mandate_id: &str, row: &ActionRow) {
        self.rows.push((mandate_id.into(), row.clone()));
    }
}

fn no_spec(_: &str) -> Option<String> {
    None
}

struct Fixture {
    root: tempfile::TempDir,
    mandates: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            mandates: tempfile::tempdir().unwrap(),
        }
    }

    fn run_dir(&self, run: &str) -> PathBuf {
        let d = self.root.path().join(".aida/relay").join(run);
        std::fs::create_dir_all(d.join("sessions")).unwrap();
        d
    }

    fn inputs<'a>(
        &'a self,
        clock: &'a dyn Fn() -> DateTime<Utc>,
        lanes: Vec<Lane>,
        principal: &'a str,
    ) -> TickInputs<'a> {
        TickInputs {
            project_root: self.root.path(),
            mandate_dir: self.mandates.path(),
            principal,
            clock,
            gates: Ok(HumanGates::default()),
            lanes,
            held_prs: Vec::new(),
            advisor_live: true,
            inherited_session_env: false,
            spec_always_human: &no_spec,
        }
    }
}

fn now_clock() -> DateTime<Utc> {
    t0() + Duration::hours(1)
}

fn outcomes(report: &TickReport) -> Vec<(String, Outcome)> {
    report
        .rows
        .iter()
        .map(|r| (r.lane.clone(), r.outcome))
        .collect()
}

// ── T7.4: blocked relays ────────────────────────────────────────────────────

#[test]
fn blocked_due_item_carries_resolving_action_and_no_restart() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let run_dir = fx.run_dir("run-b");
    let handoff = run_dir.join("sessions/h.md");
    std::fs::write(
        &handoff,
        "# handoff\nBLOCKED_ON_HUMAN: waiting on advisor signoff of the STORY-9 sketch\n",
    )
    .unwrap();
    let status_body = "state=blocked\nexit=3\n";
    std::fs::write(run_dir.join("relay.status"), status_body).unwrap();
    let before: Vec<_> = std::fs::read_dir(&run_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    let lanes = vec![lane(
        "run-b",
        LaneClass::Blocked,
        &run_dir,
        &[("state", "blocked"), ("exit", "3")],
    )];
    let mut fake = Fake::default();
    let report = run_tick(
        &fx.inputs(&now_clock, lanes, "joe"),
        &TickHooks::default(),
        &mut fake,
    );

    assert_eq!(fake.due.len(), 1, "{report:?}");
    let item = &fake.due[0].1;
    assert_eq!(item.action, DriveAction::SketchSignoff);
    assert_eq!(item.spec.as_deref(), Some("STORY-9"));
    assert!(
        item.command.contains("aida show STORY-9 --comments"),
        "{}",
        item.command
    );
    assert!(item.command.contains("bin/relay.sh"), "{}", item.command);
    assert!(
        item.command
            .contains(&format!("touch {}.done", handoff.display())),
        "{}",
        item.command
    );
    assert!(item
        .detail
        .contains("BLOCKED_ON_HUMAN: waiting on advisor signoff"));
    // Nothing was restarted: the run dir is exactly as it was.
    let after: Vec<_> = std::fs::read_dir(&run_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(before, after);
    assert_eq!(
        std::fs::read_to_string(run_dir.join("relay.status")).unwrap(),
        status_body
    );
}

#[test]
fn blocked_without_a_spec_asks_for_an_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let l = lane("r", LaneClass::Blocked, tmp.path(), &[("state", "blocked")]);
    let (action, command, _, spec) = resolving_action(&l);
    assert_eq!(action, DriveAction::Answer);
    assert_eq!(spec, None);
    assert!(command.contains("answer the question in"));
}

// ── T7.5: two concurrent ticks ──────────────────────────────────────────────

#[test]
fn concurrent_ticks_one_acts_other_skips() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let run_a = fx.run_dir("a");
    let run_b = fx.run_dir("b");
    let lanes = || {
        vec![
            lane("a", LaneClass::Crashed, &run_a, &[("state", "running")]),
            lane(
                "b",
                LaneClass::SessionCap,
                &run_b,
                &[("state", "exited"), ("exit", "0")],
            ),
        ]
    };
    let entered = std::sync::Barrier::new(2);
    let released = std::sync::Barrier::new(2);
    let (first, second) = std::thread::scope(|s| {
        let a = s.spawn(|| {
            let hook = |n: usize| {
                if n == 0 {
                    entered.wait();
                    released.wait();
                }
            };
            let mut fake = Fake {
                fail_due_for: Some("relay:b".into()),
                ..Default::default()
            };
            let r = run_tick(
                &fx.inputs(&now_clock, lanes(), "joe"),
                &TickHooks {
                    before_action: Some(&hook),
                },
                &mut fake,
            );
            (r, fake)
        });
        let b = s.spawn(|| {
            entered.wait();
            let mut fake = Fake::default();
            let r = run_tick(
                &fx.inputs(&now_clock, lanes(), "joe"),
                &TickHooks::default(),
                &mut fake,
            );
            released.wait();
            (r, fake)
        });
        (a.join().unwrap(), b.join().unwrap())
    });
    // The second tick found the lock held and did nothing but say so.
    assert_eq!(second.0.rows.len(), 1);
    assert_eq!(second.0.rows[0].outcome, Outcome::Skipped);
    assert_eq!(
        second.0.rows[0].why.as_deref(),
        Some("another drive tick holds the lock")
    );
    assert!(second.1.due.is_empty());
    // The first routed lane a; lane b's emission failed and backed off.
    assert_eq!(first.1.due.len(), 1);
    assert_eq!(
        outcomes(&first.0),
        vec![
            ("relay:a".into(), Outcome::Routed),
            ("relay:b".into(), Outcome::Refused)
        ]
    );
    // The state is shared: a third tick neither re-routes a nor retries b yet.
    let state = load_state(fx.root.path());
    let per = state.mandates.values().next().unwrap();
    assert_eq!(per.emitted.len(), 1);
    assert_eq!(per.backoff.values().next().unwrap().attempts, 1);
    let mut fake = Fake::default();
    let third = run_tick(
        &fx.inputs(&now_clock, lanes(), "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    assert!(fake.due.is_empty());
    assert_eq!(outcomes(&third), vec![("relay:b".into(), Outcome::Skipped)]);
    assert_eq!(third.rows[0].why.as_deref(), Some("backoff"));
    // One tick later (15m) the backoff has elapsed and b is routed.
    let later = || now_clock() + Duration::minutes(16);
    let mut fake = Fake::default();
    let fourth = run_tick(
        &fx.inputs(&later, lanes(), "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    assert_eq!(outcomes(&fourth), vec![("relay:b".into(), Outcome::Routed)]);
}

// ── T7.6: inherited session env ─────────────────────────────────────────────

#[test]
fn inherited_grant_is_scrubbed_and_ignored() {
    let run = |inherited: bool| {
        let fx = Fixture::new();
        drive_mandate(fx.mandates.path(), "joe");
        let d = fx.run_dir("a");
        let mut inputs = fx.inputs(
            &now_clock,
            vec![lane("a", LaneClass::Crashed, &d, &[("state", "running")])],
            "joe",
        );
        inputs.inherited_session_env = inherited;
        let mut fake = Fake::default();
        let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
        report
            .rows
            .into_iter()
            .filter(|r| r.action != "session-env")
            .map(|r| (r.lane, r.action, r.outcome))
            .collect::<Vec<_>>()
    };
    assert_eq!(run(false), run(true));
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
    inputs.inherited_session_env = true;
    let report = run_tick(&inputs, &TickHooks::default(), &mut Fake::default());
    assert_eq!(
        report.rows[0].why.as_deref(),
        Some("inherited session env ignored")
    );

    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_SESSION_GRANT", "6f1c1d6e-0000-4000-8000-0000000000aa"),
        ("AIDA_SESSION_ROLE", "advisor"),
        ("AIDA_SESSION_PURPOSE", "drive"),
    ]);
    let out = scrubbed_command("env").output().unwrap();
    let env = String::from_utf8_lossy(&out.stdout);
    for var in SESSION_ENV {
        assert!(!env.contains(&format!("{var}=")), "{var} leaked: {env}");
    }
    assert!(env.contains("AIDA_SCHEDULE_CHILD=1"));
}

// ── T7.7: a mandate that ends mid-tick ──────────────────────────────────────

fn three_lanes(fx: &Fixture) -> Vec<Lane> {
    ["a", "b", "c"]
        .iter()
        .map(|r| {
            lane(
                r,
                LaneClass::Crashed,
                &fx.run_dir(r),
                &[("state", "running")],
            )
        })
        .collect()
}

#[test]
fn mandate_stopped_mid_tick_refuses_remaining() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let dir = fx.mandates.path().to_path_buf();
    // Side effects: 0 = lease expiry, 1 = lane a, 2 = lane b.
    let hook = move |n: usize| {
        if n == 2 {
            crate::mandate::stop(&dir, &ok_human, "joe", now_clock()).unwrap();
        }
    };
    let mut fake = Fake::default();
    let report = run_tick(
        &fx.inputs(&now_clock, three_lanes(&fx), "joe"),
        &TickHooks {
            before_action: Some(&hook),
        },
        &mut fake,
    );
    assert_eq!(fake.due.len(), 1);
    assert_eq!(
        outcomes(&report),
        vec![
            ("relay:a".into(), Outcome::Routed),
            ("relay:b".into(), Outcome::Refused),
            ("relay:c".into(), Outcome::Skipped),
        ]
    );
    assert_eq!(
        report.rows[1].why.as_deref(),
        Some("mandate stopped mid-tick")
    );
    assert_eq!(
        report.rows[2].why.as_deref(),
        Some("mandate stopped mid-tick")
    );
}

#[test]
fn mandate_expired_mid_tick_refuses_remaining() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let now = Cell::new(now_clock());
    let clock = || now.get();
    let hook = |n: usize| {
        if n == 2 {
            now.set(t0() + Duration::hours(9));
        }
    };
    let mut fake = Fake::default();
    let report = run_tick(
        &fx.inputs(&clock, three_lanes(&fx), "joe"),
        &TickHooks {
            before_action: Some(&hook),
        },
        &mut fake,
    );
    assert_eq!(fake.due.len(), 1);
    assert_eq!(report.rows[1].outcome, Outcome::Refused);
    assert_eq!(
        report.rows[1].why.as_deref(),
        Some("mandate expired mid-tick")
    );
    assert_eq!(report.rows[2].outcome, Outcome::Skipped);
}

// ── T7.8: dedup across ticks ────────────────────────────────────────────────

#[test]
fn due_items_dedup_across_two_ticks() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let d = fx.run_dir("a");
    let lanes = |updated: &str| {
        vec![lane(
            "a",
            LaneClass::SessionCap,
            &d,
            &[("state", "exited"), ("exit", "0"), ("updated", updated)],
        )]
    };
    let mut fake = Fake::default();
    run_tick(
        &fx.inputs(&now_clock, lanes("u1"), "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    run_tick(
        &fx.inputs(&now_clock, lanes("u1"), "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    assert_eq!(
        fake.due.len(),
        1,
        "the same (lane, action, head) routes once"
    );
    // A new block on the same run (a new `updated`) is a new item.
    run_tick(
        &fx.inputs(&now_clock, lanes("u2"), "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    assert_eq!(fake.due.len(), 2);
    assert_ne!(fake.due[0].1.key(), fake.due[1].1.key());
}

// ── T7.9 / T7.11: gate layers ───────────────────────────────────────────────

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn repo_with_committed_gates(ops: &str) -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(repo.path().join(".aida")).unwrap();
    std::fs::write(
        repo.path().join(".aida/config.toml"),
        format!("[mandate.human_gates]\nops = {ops}\n"),
    )
    .unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "gates"]);
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    repo
}

#[test]
fn gate_removed_in_working_tree_still_enforced_from_origin_main() {
    let repo = repo_with_committed_gates("[\"dispatch\"]");
    // An agent loosens the gate in the working tree, and even commits it on
    // a branch: origin/main is still the committed layer.
    std::fs::write(
        repo.path().join(".aida/config.toml"),
        "[mandate.human_gates]\nops = []\n",
    )
    .unwrap();
    let committed = crate::mandate_gates::committed_layer(repo.path()).unwrap();
    assert!(committed.ops.contains(&GateOp::Dispatch));
    git(repo.path(), &["commit", "-qam", "loosen"]);
    let committed = crate::mandate_gates::committed_layer(repo.path()).unwrap();
    assert!(committed.ops.contains(&GateOp::Dispatch));

    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let d = fx.run_dir("a");
    let mut inputs = fx.inputs(
        &now_clock,
        vec![lane(
            "a",
            LaneClass::SessionCap,
            &d,
            &[("state", "exited"), ("exit", "0")],
        )],
        "joe",
    );
    inputs.gates = Ok(crate::mandate_gates::union(committed, Default::default()));
    let mut fake = Fake::default();
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert!(
        fake.due.is_empty(),
        "a gated dispatch never reaches the advisor"
    );
    assert_eq!(outcomes(&report), vec![("relay:a".into(), Outcome::Gated)]);
    assert_eq!(fake.notices.len(), 1);
}

#[test]
fn a_tree_without_config_is_an_empty_committed_layer() {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(repo.path().join("README"), "x").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-q", "-m", "init"]);
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    assert_eq!(
        crate::mandate_gates::committed_layer(repo.path()).unwrap(),
        Default::default()
    );
}

#[test]
fn unreadable_gate_blob_gates_everything() {
    let not_a_repo = tempfile::tempdir().unwrap();
    let gates = crate::mandate_gates::committed_layer(not_a_repo.path());
    assert!(gates.is_err());
    let gates: std::result::Result<HumanGates, String> = Err("no origin/main".into());
    for op in [GateOp::Ship, GateOp::SketchSignoff, GateOp::Dispatch] {
        assert!(crate::mandate_gates::gate_reason(&gates, op)
            .unwrap()
            .contains("gate config unreadable"));
    }
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let d = fx.run_dir("a");
    let mut inputs = fx.inputs(
        &now_clock,
        vec![lane("a", LaneClass::Crashed, &d, &[("state", "running")])],
        "joe",
    );
    inputs.gates = gates;
    let mut fake = Fake::default();
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert_eq!(outcomes(&report), vec![("relay:a".into(), Outcome::Gated)]);
    assert!(fake.due.is_empty());
}

// ── T7.10 / T7.11: the ship path ────────────────────────────────────────────

fn ship_check<'a>(
    state: &'a MandateState,
    policy: &'a crate::pr_ship::ShipApprovalPolicy,
    ledger: &'a std::result::Result<
        std::collections::BTreeMap<String, crate::mandate::Exclusion>,
        String,
    >,
    hold: &'a crate::mandate::HoldFacts,
    changed_paths: Option<&'a [String]>,
) -> crate::mandate::ShipCheck<'a> {
    crate::mandate::ShipCheck {
        state,
        now: now_clock(),
        grant: Some(("advisor", "joe")),
        specs: &[],
        specs_resolved: true,
        candidates: &[],
        policy,
        head: None,
        override_stale_approval: false,
        ledger,
        hold,
        changed_paths,
        ship_gate: None,
    }
}

#[test]
fn pr_touching_gate_paths_is_gated() {
    let tmp = tempfile::tempdir().unwrap();
    let state = MandateState::Active(drive_mandate(tmp.path(), "joe"));
    let policy = Default::default();
    let ledger = Ok(Default::default());
    let hold = crate::mandate::HoldFacts::None;
    for path in [
        ".aida/config.toml",
        "aida-cli-lib/src/mandate.rs",
        "aida-cli-lib/src/mandate_ship.rs",
    ] {
        let paths = vec!["README.md".to_string(), path.to_string()];
        let c = ship_check(&state, &policy, &ledger, &hold, Some(&paths));
        assert_eq!(
            crate::mandate::ship_decision(&c),
            Err(MandateRefusal::TouchesGates { path: path.into() })
        );
    }
    // pr_cmd.rs is NOT protected as a whole (condition C1).
    assert_eq!(
        crate::mandate_gates::touches_gates(&["aida-cli-lib/src/pr_cmd.rs".into()]),
        None
    );
    // `ship` listed as a human gate refuses every mandate ship.
    let paths = vec!["README.md".to_string()];
    let mut c = ship_check(&state, &policy, &ledger, &hold, Some(&paths));
    c.ship_gate = Some("`ship` is listed in [mandate.human_gates]".into());
    assert!(matches!(
        crate::mandate::ship_decision(&c),
        Err(MandateRefusal::HumanGated { .. })
    ));
    // The tick classifies such a PR as gated, never a ship for the advisor.
    let pr = HeldPr {
        pr: 7,
        releasable: true,
        head: Some("a".repeat(40)),
        spec: None,
        approved_at_head: true,
        changed_paths: Some(vec![".aida/config.toml".into()]),
    };
    let LaneDecision::Act(p) = plan_held_pr(&pr) else {
        panic!("an approved held PR is a lane")
    };
    assert!(p.forced_gate.unwrap().contains(".aida/config.toml"));
}

#[test]
fn unresolved_changed_paths_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let state = MandateState::Active(drive_mandate(tmp.path(), "joe"));
    let policy = Default::default();
    let ledger = Ok(Default::default());
    let hold = crate::mandate::HoldFacts::None;
    let c = ship_check(&state, &policy, &ledger, &hold, None);
    assert_eq!(
        crate::mandate::ship_decision(&c),
        Err(MandateRefusal::ChangedPathsUnresolved)
    );
    let pr = HeldPr {
        pr: 8,
        releasable: true,
        head: Some("b".repeat(40)),
        spec: None,
        approved_at_head: true,
        changed_paths: None,
    };
    let LaneDecision::Act(p) = plan_held_pr(&pr) else {
        panic!("an approved held PR is a lane")
    };
    assert!(p.forced_gate.unwrap().contains("could not be resolved"));
    // An unapproved or non-releasable hold is not a drive lane at all.
    let mut quiet = pr.clone();
    quiet.approved_at_head = false;
    assert_eq!(plan_held_pr(&quiet), LaneDecision::None);
    quiet.approved_at_head = true;
    quiet.releasable = false;
    assert_eq!(plan_held_pr(&quiet), LaneDecision::None);
}

#[test]
fn an_approved_held_pr_routes_a_ship_to_the_advisor() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
    inputs.held_prs = vec![HeldPr {
        pr: 9,
        releasable: true,
        head: Some("c".repeat(40)),
        spec: Some("TASK-1".into()),
        approved_at_head: true,
        changed_paths: Some(vec!["README.md".into()]),
    }];
    let mut fake = Fake::default();
    run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert_eq!(fake.due.len(), 1);
    assert_eq!(fake.due[0].1.command, "aida pr ship 9");
    // An operator-only spec is always gated.
    let always = |s: &str| (s == "TASK-1").then(|| "operator-only".to_string());
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
    inputs.spec_always_human = &always;
    inputs.held_prs = vec![HeldPr {
        pr: 9,
        releasable: true,
        head: Some("c".repeat(40)),
        spec: Some("TASK-1".into()),
        approved_at_head: true,
        changed_paths: Some(vec!["README.md".into()]),
    }];
    let mut fake = Fake::default();
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert!(fake.due.is_empty());
    assert_eq!(report.rows[0].outcome, Outcome::Gated);
    assert!(report.rows[0]
        .why
        .as_deref()
        .unwrap()
        .contains("TASK-1 is operator-only"));
}

// ── T7.12: principal under the scheduler ────────────────────────────────────

#[test]
fn principal_mismatch_warns_and_does_nothing() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "a");
    let d = fx.run_dir("x");
    let lanes = || vec![lane("x", LaneClass::Crashed, &d, &[("state", "running")])];
    let mut fake = Fake::default();
    let report = run_tick(
        &fx.inputs(&now_clock, lanes(), "b"),
        &TickHooks::default(),
        &mut fake,
    );
    assert_eq!(report.rows.len(), 1);
    assert_eq!(report.rows[0].outcome, Outcome::Refused);
    assert_eq!(
        report.rows[0].why.as_deref(),
        Some("principal mismatch: record=a tick=b")
    );
    assert!(fake.due.is_empty());
    assert_eq!(fake.notices.len(), 1);
    // Once per mandate id.
    let again = run_tick(
        &fx.inputs(&now_clock, lanes(), "b"),
        &TickHooks::default(),
        &mut fake,
    );
    assert!(again.rows.is_empty());
    assert_eq!(fake.notices.len(), 1);

    let repo = tempfile::tempdir().unwrap();
    assert_eq!(scheduler_principal(repo.path()), PrincipalProbe::NoDriver);
}

#[test]
fn principal_warnings_name_both_values_and_the_fix() {
    let w = principal_warnings(&PrincipalProbe::Resolved("cron".into()), "joe", true);
    assert_eq!(w.len(), 1);
    assert!(w[0].contains("`cron`") && w[0].contains("`joe`") && w[0].contains("AIDA_USER=joe"));
    assert!(principal_warnings(&PrincipalProbe::Resolved("joe".into()), "joe", true).is_empty());
    let w = principal_warnings(&PrincipalProbe::NoDriver, "joe", false);
    assert_eq!(w.len(), 2);
    assert!(w[0].contains("no schedule driver"));
    assert!(w[1].contains("fleet drive --tick"));
    assert_eq!(
        aida_user_in("Environment=\"AIDA_USER=joe\" PATH=/bin"),
        Some("joe".into())
    );
    assert_eq!(
        aida_user_in("*/5 * * * * AIDA_USER=joe aida schedule tick # aida-schedule-tick:/r"),
        Some("joe".into())
    );
    assert_eq!(aida_user_in("ExecStart=/bin/aida schedule tick"), None);
}

// ── T7.13: fail-closed parsing ──────────────────────────────────────────────

fn write_record(dir: &Path, body: &serde_json::Value) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let id = body["id"].as_str().unwrap();
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join(format!("{id}.json")))
        .unwrap();
    f.write_all(body.to_string().as_bytes()).unwrap();
}

#[test]
fn unknown_scope_string_fails_closed() {
    for bogus in ["relay-restart", "bogus"] {
        let dir = tempfile::tempdir().unwrap();
        let mut body = serde_json::to_value(drive_mandate(dir.path(), "joe")).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        body["id"] = id.clone().into();
        body["scopes"] = serde_json::json!([bogus]);
        body["issued_at"] = (t0() + Duration::minutes(1)).to_rfc3339().into();
        write_record(dir.path(), &body);
        let (valid, warnings) = crate::mandate::load_valid(dir.path(), "joe", now_clock());
        assert!(
            valid.iter().all(|m| m.id != id),
            "{bogus} record must be ignored"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains(&id) && w.contains("unreadable")),
            "{warnings:?}"
        );
    }
}

#[test]
fn unknown_human_gate_op_is_config_error() {
    use crate::mandate_gates::parse_layer;
    assert!(
        parse_layer("[mandate.human_gates]\nops = [\"bogus\"]\n", "t")
            .unwrap_err()
            .to_string()
            .contains("unknown op `bogus`")
    );
    assert!(parse_layer("[mandate.human_gates]\nopz = []\n", "t").is_err());
    assert!(parse_layer("[mandate.other]\nx = 1\n", "t").is_err());
    assert!(parse_layer("[mandate.drive]\nmax_actions_per_tick = 0\n", "t").is_err());
    let layer = parse_layer(
        "[mandate.human_gates]\nops = [\"ship\", \"hold-clear\"]\n[mandate.drive]\nmax_actions_per_tick = 3\n",
        "t",
    )
    .unwrap();
    assert_eq!(
        layer.ops,
        [GateOp::Ship, GateOp::HoldClear].into_iter().collect()
    );
    assert_eq!(layer.max_actions_per_tick, Some(3));
    assert_eq!(
        parse_layer("[other]\nx = 1\n", "t").unwrap(),
        Default::default()
    );
    // Only the operator layer sets the budget; layers only add gates.
    let committed = parse_layer(
        "[mandate.human_gates]\nops = [\"dispatch\"]\n[mandate.drive]\nmax_actions_per_tick = 99\n",
        "t",
    )
    .unwrap();
    let operator = parse_layer("[mandate.human_gates]\nops = [\"ship\"]\n", "t").unwrap();
    let g = crate::mandate_gates::union(committed, operator);
    assert_eq!(g.max_actions_per_tick, None);
    assert_eq!(g.describe(), "ship, dispatch");
}

// ── T7.14: kept from revision 1 ─────────────────────────────────────────────

#[test]
fn a_story_1698_record_round_trips_unchanged() {
    let old = serde_json::json!({
        "id": "6f1c1d6e-0000-4000-8000-000000000001",
        "principal": "joe",
        "seat": "advisor",
        "scope": "ship-held",
        "include_supervised": false,
        "issued_at": "2026-10-17T12:00:00Z",
        "expires_at": "2026-10-17T20:00:00Z",
    });
    let m: Mandate = serde_json::from_value(old.clone()).unwrap();
    assert!(!m.is_drive());
    assert_eq!(
        m.effective_scopes(),
        [Scope::ShipHeld].into_iter().collect()
    );
    let back = serde_json::to_value(&m).unwrap();
    assert_eq!(back, old, "no new keys are written for a ship-held record");
}

#[test]
fn a_drive_record_loads_in_the_story_1698_loader_as_ship_held() {
    /// The record shape #2687 shipped (no `scopes`, no `kind`).
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct Story1698Mandate {
        id: String,
        principal: String,
        seat: String,
        scope: String,
        #[serde(default)]
        include_supervised: bool,
        issued_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    }
    let dir = tempfile::tempdir().unwrap();
    let m = drive_mandate(dir.path(), "joe");
    let text = std::fs::read_to_string(dir.path().join(format!("{}.json", m.id))).unwrap();
    let old: Story1698Mandate = serde_json::from_str(&text).unwrap();
    assert_eq!(old.scope, crate::mandate::SCOPE_SHIP_HELD);
    assert!(old.include_supervised);
    let (valid, warnings) = crate::mandate::load_valid(dir.path(), "joe", now_clock());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(valid[0].is_drive());
    assert_eq!(
        valid[0].effective_scopes(),
        Scope::ALL.into_iter().collect()
    );
}

#[test]
fn include_supervised_defaults_on_with_drive() {
    use crate::mandate::drive_includes_supervised as inc;
    assert!(inc(true, false, false));
    assert!(!inc(true, false, true));
    assert!(!inc(false, false, false));
    assert!(inc(false, true, false));
    use clap::Parser;
    let parse = |args: &[&str]| crate::cli::Cli::try_parse_from(args);
    assert!(parse(&["aida", "mandate", "start", "--drive"]).is_ok());
    assert!(parse(&[
        "aida",
        "mandate",
        "start",
        "--drive",
        "--exclude-supervised"
    ])
    .is_ok());
    assert!(parse(&[
        "aida",
        "mandate",
        "start",
        "--ship-held",
        "--exclude-supervised"
    ])
    .is_err());
    assert!(parse(&["aida", "mandate", "status", "--line"]).is_ok());
    assert!(parse(&["aida", "fleet", "drive", "--tick"]).is_ok());
}

#[test]
fn drive_start_and_stop_refuse_non_tty() {
    let dir = tempfile::tempdir().unwrap();
    let err = crate::mandate::start_with(
        dir.path(),
        &no_tty,
        "joe",
        Duration::hours(1),
        None,
        true,
        true,
        t0(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("human at an interactive terminal"));
    drive_mandate(dir.path(), "joe");
    assert!(crate::mandate::stop(dir.path(), &no_tty, "joe", now_clock()).is_err());
}

#[test]
fn line_and_notice_lines() {
    let dir = tempfile::tempdir().unwrap();
    let m = drive_mandate(dir.path(), "joe");
    let state = MandateState::Active(m.clone());
    assert_eq!(
        crate::mandate::status_line(&state, t0() + Duration::minutes(140), "none").unwrap(),
        "MANDATE drive · 5h40m left · by joe · fire · gates: none"
    );
    assert_eq!(
        crate::mandate::notice_line(dir.path(), &state, now_clock(), "advisor", "ship"),
        crate::mandate::status_line(&state, now_clock(), "ship")
    );
    // Stopped: one line per seat, then silence.
    crate::mandate::stop(dir.path(), &ok_human, "joe", now_clock()).unwrap();
    let (valid, _) = crate::mandate::load_valid(dir.path(), "joe", now_clock());
    let stopped = crate::mandate::state_at(&valid, now_clock());
    assert!(matches!(stopped, MandateState::Stopped(_)));
    let first = crate::mandate::notice_line(dir.path(), &stopped, now_clock(), "advisor", "");
    assert!(first
        .unwrap()
        .contains(&format!("MANDATE {} was stopped", m.id)));
    assert_eq!(
        crate::mandate::notice_line(dir.path(), &stopped, now_clock(), "advisor", ""),
        None
    );
    assert!(
        crate::mandate::notice_line(dir.path(), &stopped, now_clock(), "implementer", "").is_some()
    );
    // The seen markers never confuse the loader.
    let (valid, warnings) = crate::mandate::load_valid(dir.path(), "joe", now_clock());
    assert_eq!(valid.len(), 1);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn handoff_show_order_is_mandate_then_standing_orders_then_handoff() {
    let out = crate::seat_rotation::compose_handoff_show_with_mandate(
        Some("MANDATE drive · 1h left · by joe · gates: none"),
        Some("STANDING ORDERS (x)\nobey"),
        Some("handoff body\n"),
        "advisor",
    );
    let m = out.find("ACTIVE MANDATE").unwrap();
    let s = out.find("STANDING ORDERS").unwrap();
    let h = out.find("handoff body").unwrap();
    assert!(m < s && s < h, "{out}");
    assert_eq!(
        crate::seat_rotation::compose_handoff_show_with_mandate(None, None, Some("b\n"), "advisor"),
        "b\n"
    );
}

#[test]
fn one_operator_escalation_per_absence_episode() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let tick = |live: bool, fake: &mut Fake| {
        let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
        inputs.advisor_live = live;
        run_tick(&inputs, &TickHooks::default(), fake)
    };
    let mut fake = Fake::default();
    let r = tick(false, &mut fake);
    assert_eq!(r.rows[0].action, "escalate");
    tick(false, &mut fake);
    assert_eq!(fake.notices.len(), 1, "one notice per episode");
    tick(true, &mut fake);
    tick(false, &mut fake);
    assert_eq!(fake.notices.len(), 2, "a new episode notifies again");
}

#[test]
fn mandate_drive_active_predicate_field_type_checks() {
    use crate::schedule_predicate::parse;
    assert!(parse("mandate.drive_active == true").is_ok());
    assert!(parse("mandate.drive_active").is_ok());
    assert!(parse("mandate.drive_active && drain.lock_free").is_ok());
    assert!(parse("mandate.drive_active > 1").is_err());
    assert!(parse("mandate.drive_active == 3").is_err());
}

#[test]
fn mandate_drive_action_is_emitted_for_every_outcome() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let lanes = vec![
        lane(
            "routed",
            LaneClass::Crashed,
            &fx.run_dir("routed"),
            &[("state", "running")],
        ),
        lane(
            "refused",
            LaneClass::Crashed,
            &fx.run_dir("refused"),
            &[("state", "running")],
        ),
        lane(
            "skipped",
            LaneClass::SignalStopped,
            &fx.run_dir("skipped"),
            &[("exit", "130")],
        ),
    ];
    let mut inputs = fx.inputs(&now_clock, lanes, "joe");
    inputs.held_prs = vec![HeldPr {
        pr: 5,
        releasable: true,
        head: Some("d".repeat(40)),
        spec: None,
        approved_at_head: true,
        changed_paths: None,
    }];
    let mut fake = Fake {
        reaped: 2,
        fail_due_for: Some("relay:refused".into()),
        ..Default::default()
    };
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    let seen: BTreeSet<&str> = fake.rows.iter().map(|(_, r)| r.outcome.as_str()).collect();
    assert_eq!(
        seen,
        ["done", "routed", "gated", "refused", "skipped"]
            .into_iter()
            .collect()
    );
    assert_eq!(fake.rows.len(), report.rows.len(), "every row is ledgered");
    assert!(fake.rows.iter().all(|(id, _)| !id.is_empty()));

    // The production ledger writes MandateDriveAction events the status and
    // the digest read back.
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    for (_, row) in &fake.rows {
        emit_action(root.path(), "m-1", "tick", row);
    }
    emit_session_ship(root.path(), "m-1", 12, Outcome::Done, None);
    let rows = ledger_rows(root.path(), "m-1");
    assert_eq!(rows.len(), fake.rows.len() + 1);
    assert_eq!(rows.last().unwrap().1, "session");
    let text = ledger_text(root.path(), "m-1").unwrap();
    assert!(text.contains("mandate-driven actions:"), "{text}");
    let lines = digest_lines(
        root.path(),
        Utc::now() - Duration::hours(1),
        Utc::now() + Duration::hours(1),
    );
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("mandate m-1: "), "{lines:?}");
    assert!(ledger_rows(root.path(), "other").is_empty());
}

#[test]
fn budget_caps_actions_per_tick() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let lanes: Vec<Lane> = (0..4)
        .map(|i| {
            let r = format!("r{i}");
            lane(
                &r,
                LaneClass::Crashed,
                &fx.run_dir(&r),
                &[("state", "running")],
            )
        })
        .collect();
    let mut inputs = fx.inputs(&now_clock, lanes, "joe");
    inputs.gates = Ok(HumanGates {
        max_actions_per_tick: Some(2),
        ..Default::default()
    });
    let mut fake = Fake::default();
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert_eq!(fake.due.len(), 2);
    assert_eq!(
        report
            .rows
            .iter()
            .filter(|r| r.outcome == Outcome::Skipped)
            .count(),
        2
    );
}

#[test]
fn signal_stops_and_duplicate_launches_are_skipped_once() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let lanes = || {
        vec![
            lane(
                "s",
                LaneClass::SignalStopped,
                &fx.run_dir("s"),
                &[("exit", "143")],
            ),
            lane(
                "l",
                LaneClass::RunLocked,
                &fx.run_dir("l"),
                &[("exit", "6")],
            ),
        ]
    };
    let mut fake = Fake::default();
    let first = run_tick(
        &fx.inputs(&now_clock, lanes(), "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    assert_eq!(first.rows.len(), 2);
    assert!(first.rows.iter().all(|r| r.outcome == Outcome::Skipped));
    let second = run_tick(
        &fx.inputs(&now_clock, lanes(), "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    assert!(second.rows.is_empty());
}

#[test]
fn advisor_seat_and_credit_lanes_route_to_the_operator() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let mut advisor = lane(
        "adv",
        LaneClass::Crashed,
        &fx.run_dir("adv"),
        &[("state", "running")],
    );
    advisor.seat_kind = SeatKind::Advisor;
    let credits = lane(
        "api",
        LaneClass::TerminalApi,
        &fx.run_dir("api"),
        &[("exit", "7")],
    );
    let mut fake = Fake::default();
    let report = run_tick(
        &fx.inputs(&now_clock, vec![advisor, credits], "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    assert!(fake.due.is_empty());
    assert_eq!(fake.notices.len(), 2);
    assert!(report.rows.iter().all(|r| r.outcome == Outcome::Routed));
}

#[test]
fn no_drive_mandate_means_an_idle_tick() {
    let fx = Fixture::new();
    // A ship-held (not drive) mandate does not drive.
    crate::mandate::start(
        fx.mandates.path(),
        &ok_human,
        "joe",
        Duration::hours(8),
        None,
        false,
        t0(),
    )
    .unwrap();
    let d = fx.run_dir("a");
    let mut fake = Fake::default();
    let report = run_tick(
        &fx.inputs(
            &now_clock,
            vec![lane("a", LaneClass::Crashed, &d, &[])],
            "joe",
        ),
        &TickHooks::default(),
        &mut fake,
    );
    assert_eq!(report.idle.as_deref(), Some("no active drive mandate"));
    assert!(report.rows.is_empty() && fake.due.is_empty() && fake.notices.is_empty());
}

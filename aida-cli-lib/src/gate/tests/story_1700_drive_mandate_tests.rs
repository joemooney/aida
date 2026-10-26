//! Tests for STORY-1700: the drive mandate (ADR-77) — the record, the
//! display lines, the human gates, the drive tick (R2-R6) and its ledger.
//! T7.4-T7.14 of `docs/plans/2026-10-15-story-1700-drive-mandate.md`.
//
// trace:STORY-1700 | ai:claude

use super::*;
use crate::gate::fleet_lanes::{LaneEvidence, SeatKind};
use crate::mandate_drive::*;
use anyhow::Result;
use std::cell::Cell;
use std::collections::BTreeSet;

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
    crate::gate::mandate::start_with(
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
    // The run record names the spec the run works (R3-2).
    map.entry("ticket".into())
        .or_insert_with(|| "TASK-77".into());
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
    /// Further reap rows (skips, failures) the next reap returns.
    reap: Vec<ReapRow>,
    fail_due_for: Option<String>,
    /// Fail this many operator notices before delivering.
    fail_notices: usize,
    /// For this lane, write the due event then fail the mail.
    // trace:TASK-1955 | ai:claude (TASK-2084)
    fail_mail_for: Option<String>,
    /// `event_recorded` as each `emit_due` call received it.
    recorded_on_entry: Vec<bool>,
}

impl DriveEffects for Fake {
    fn emit_due(
        &mut self,
        mandate_id: &str,
        item: &Planned,
        event_recorded: &mut bool,
    ) -> Result<()> {
        self.recorded_on_entry.push(*event_recorded);
        if self.fail_due_for.as_deref() == Some(item.lane.as_str()) {
            anyhow::bail!("events.jsonl is read-only");
        }
        *event_recorded = true;
        if self.fail_mail_for.as_deref() == Some(item.lane.as_str()) {
            anyhow::bail!("mailbox is read-only");
        }
        self.due.push((mandate_id.into(), item.clone()));
        Ok(())
    }
    fn notify_operator(&mut self, title: &str, body: &str) -> Result<String> {
        if self.fail_notices > 0 {
            self.fail_notices -= 1;
            anyhow::bail!("notify command exited 1");
        }
        self.notices.push((title.into(), body.into()));
        Ok("sent".into())
    }
    fn reap_dead_leases(&mut self) -> Vec<ReapRow> {
        let mut rows: Vec<ReapRow> = (0..std::mem::take(&mut self.reaped))
            .map(|i| ReapRow {
                subject: format!("session s{i}"),
                outcome: Outcome::Done,
                why: "reaped — lease released".into(),
            })
            .collect();
        rows.append(&mut self.reap);
        rows
    }
    fn ledger(&mut self, mandate_id: &str, row: &ActionRow) {
        self.rows.push((mandate_id.into(), row.clone()));
    }
}

fn no_spec(_: &str) -> Option<String> {
    None
}

/// The tick fixture's store: it knows the lane default `TASK-77` (no
/// signoff pending) and nothing else.
// trace:TASK-1955 | ai:claude (TASK-1925)
fn fixture_signoff(id: &str) -> Option<bool> {
    (id == "TASK-77").then_some(false)
}

fn story_9_awaits(id: &str) -> Option<bool> {
    (id == "STORY-9").then_some(true)
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
            spec_signoff: &fixture_signoff,
            store_readable: true,
            unowned_specs: Vec::new(),
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

// trace:STORY-1700.ace038d6 | ai:claude
// trace:STORY-1700.acb260c7 | ai:claude
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
    let mut inputs = fx.inputs(&now_clock, lanes, "joe");
    inputs.spec_signoff = &story_9_awaits;
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);

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
    let (action, command, _, spec) = resolving_action(&l, &fixture_signoff);
    assert_eq!(action, DriveAction::Answer);
    assert!(spec.is_empty());
    assert!(command.contains("answer the question in"));
}

// ── T7.5: two concurrent ticks ──────────────────────────────────────────────

// trace:STORY-1700.acb260c7 | ai:claude
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
                    ..Default::default()
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
    let state = load_state(fx.root.path()).unwrap();
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

// trace:STORY-1700.ac8eabb8 | ai:claude
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

// trace:STORY-1700.acb260c7 | ai:claude
#[test]
fn mandate_stopped_mid_tick_refuses_remaining() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let dir = fx.mandates.path().to_path_buf();
    // Side effects: 0 = lease expiry, 1 = lane a, 2 = lane b.
    let hook = move |n: usize| {
        if n == 2 {
            crate::gate::mandate::stop(&dir, &ok_human, "joe", now_clock()).unwrap();
        }
    };
    let mut fake = Fake::default();
    let report = run_tick(
        &fx.inputs(&now_clock, three_lanes(&fx), "joe"),
        &TickHooks {
            before_action: Some(&hook),
            ..Default::default()
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

// trace:STORY-1700.acb260c7 | ai:claude
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
            ..Default::default()
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

// trace:STORY-1700.acb260c7 | ai:claude
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
    let out = crate::test_git::git_command()
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

// trace:STORY-1700.ac2ed84c | ai:claude
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
    let committed = crate::gate::mandate_gates::committed_layer(repo.path()).unwrap();
    assert!(committed.ops.contains(&GateOp::Dispatch));
    git(repo.path(), &["commit", "-qam", "loosen"]);
    let committed = crate::gate::mandate_gates::committed_layer(repo.path()).unwrap();
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
    inputs.gates = Ok(crate::gate::mandate_gates::union(
        committed,
        Default::default(),
    ));
    let mut fake = Fake::default();
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert!(
        fake.due.is_empty(),
        "a gated dispatch never reaches the advisor"
    );
    assert_eq!(outcomes(&report), vec![("relay:a".into(), Outcome::Gated)]);
    assert_eq!(fake.notices.len(), 1);
}

// trace:STORY-1700.ac2ed84c | ai:claude
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
        crate::gate::mandate_gates::committed_layer(repo.path()).unwrap(),
        Default::default()
    );
}

// trace:STORY-1700.ac2ed84c | ai:claude
#[test]
fn unreadable_gate_blob_gates_everything() {
    let not_a_repo = tempfile::tempdir().unwrap();
    let gates = crate::gate::mandate_gates::committed_layer(not_a_repo.path());
    assert!(gates.is_err());
    let gates: std::result::Result<HumanGates, String> = Err("no origin/main".into());
    for op in [GateOp::Ship, GateOp::SketchSignoff, GateOp::Dispatch] {
        assert!(crate::gate::mandate_gates::gate_reason(&gates, op)
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
    policy: &'a crate::gate::ship::ShipApprovalPolicy,
    ledger: &'a std::result::Result<
        std::collections::BTreeMap<String, crate::gate::mandate::Exclusion>,
        String,
    >,
    hold: &'a crate::gate::mandate::HoldFacts,
    changed_paths: Option<&'a [String]>,
) -> crate::gate::mandate::ShipCheck<'a> {
    crate::gate::mandate::ShipCheck {
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
        carry_over: None,
        changed_paths,
        protected: crate::gate::mandate_gates::ProtectedPaths::floor_ref(),
        ship_gate: None,
        hold_clear_gate: None,
    }
}

// trace:STORY-1700.ac8eabb8 | ai:claude
#[test]
fn pr_touching_gate_paths_is_gated() {
    let tmp = tempfile::tempdir().unwrap();
    let state = MandateState::Active(drive_mandate(tmp.path(), "joe"));
    let policy = Default::default();
    let ledger = Ok(Default::default());
    let hold = crate::gate::mandate::HoldFacts::None;
    for path in [".aida/config.toml", "aida-cli-lib/src/dispatch.rs"] {
        let paths = vec!["README.md".to_string(), path.to_string()];
        let c = ship_check(&state, &policy, &ledger, &hold, Some(&paths));
        assert_eq!(
            crate::gate::mandate::ship_decision(&c),
            Err(MandateRefusal::TouchesGates(
                crate::gate::mandate_gates::GateMatch {
                    path: path.into(),
                    entry: path.into(),
                    source: crate::gate::mandate_gates::PathSource::BuiltIn,
                }
            ))
        );
    }
    for path in [
        "aida-cli-lib/src/gate/mandate.rs",
        "aida-cli-lib/src/gate/mandate_drive.rs",
        "aida-cli-lib/src/gate/mandate_ship.rs",
        "aida-cli-lib/src/gate/mandate_traces.rs",
        "aida-cli-lib/src/gate/pr_ship_batch.rs",
        "aida-cli-lib/src/gate/relay_launch.rs",
        "aida-cli-lib/src/gate/rework_hold_release.rs",
        "aida-cli-lib/src/gate/merge_hold_core.rs",
        "aida-cli-lib/src/gate/review_authority.rs",
        "aida-cli-lib/src/gate/solo_review.rs",
        "aida-cli-lib/src/gate/review_verdict.rs",
    ] {
        let paths = vec!["README.md".to_string(), path.to_string()];
        let c = ship_check(&state, &policy, &ledger, &hold, Some(&paths));
        assert_eq!(
            crate::gate::mandate::ship_decision(&c),
            Err(MandateRefusal::TouchesGates(
                crate::gate::mandate_gates::GateMatch {
                    path: path.into(),
                    entry: "aida-cli-lib/src/gate/".into(),
                    source: crate::gate::mandate_gates::PathSource::BuiltIn,
                }
            ))
        );
    }
    // pr_cmd.rs is NOT protected as a whole (condition C1).
    assert_eq!(
        crate::gate::mandate_gates::touches_gates(
            &["aida-cli-lib/src/pr_cmd.rs".into()],
            crate::gate::mandate_gates::ProtectedPaths::floor_ref()
        ),
        None
    );
    // `ship` listed as a human gate refuses every mandate ship.
    let paths = vec!["README.md".to_string()];
    let mut c = ship_check(&state, &policy, &ledger, &hold, Some(&paths));
    c.ship_gate = Some("`ship` is listed in [mandate.human_gates]".into());
    assert!(matches!(
        crate::gate::mandate::ship_decision(&c),
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
    let LaneDecision::Act(p) =
        plan_held_pr(&pr, crate::gate::mandate_gates::ProtectedPaths::floor_ref())
    else {
        panic!("an approved held PR is a lane")
    };
    assert!(p.forced_gate.unwrap().contains(".aida/config.toml"));
}

// trace:STORY-1700.ac8eabb8 | ai:claude
#[test]
fn unresolved_changed_paths_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let state = MandateState::Active(drive_mandate(tmp.path(), "joe"));
    let policy = Default::default();
    let ledger = Ok(Default::default());
    let hold = crate::gate::mandate::HoldFacts::None;
    let c = ship_check(&state, &policy, &ledger, &hold, None);
    assert_eq!(
        crate::gate::mandate::ship_decision(&c),
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
    let LaneDecision::Act(p) =
        plan_held_pr(&pr, crate::gate::mandate_gates::ProtectedPaths::floor_ref())
    else {
        panic!("an approved held PR is a lane")
    };
    assert!(p.forced_gate.unwrap().contains("could not be resolved"));
    // An unapproved or non-releasable hold is not a drive lane at all.
    let mut quiet = pr.clone();
    quiet.approved_at_head = false;
    assert_eq!(
        plan_held_pr(
            &quiet,
            crate::gate::mandate_gates::ProtectedPaths::floor_ref()
        ),
        LaneDecision::None
    );
    quiet.approved_at_head = true;
    quiet.releasable = false;
    assert_eq!(
        plan_held_pr(
            &quiet,
            crate::gate::mandate_gates::ProtectedPaths::floor_ref()
        ),
        LaneDecision::None
    );
}

// trace:STORY-1700.ace038d6 | ai:claude
// trace:STORY-1700.ac2ed84c | ai:claude
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

/// The floor plus an operator-layer `x/` prefix (STORY-1888).
fn operator_protected() -> crate::gate::mandate_gates::ProtectedPaths {
    crate::gate::mandate_gates::union(
        Default::default(),
        crate::gate::mandate_gates::parse_layer(
            "[mandate.human_gates]\nprotected_paths = [\"x/\"]\n",
            "~/.aida/config.toml",
        )
        .unwrap(),
    )
    .protected
}

// trace:STORY-1888.acd4bc79 | ai:claude
#[test]
fn operator_layer_path_refuses_mandate_ship() {
    let tmp = tempfile::tempdir().unwrap();
    let state = MandateState::Active(drive_mandate(tmp.path(), "joe"));
    let policy = Default::default();
    let ledger = Ok(Default::default());
    let hold = crate::gate::mandate::HoldFacts::None;
    let protected = operator_protected();
    let paths = vec!["README.md".to_string(), "x/y.rs".to_string()];
    let mut c = ship_check(&state, &policy, &ledger, &hold, Some(&paths));
    c.protected = &protected;
    let hit = crate::gate::mandate_gates::GateMatch {
        path: "x/y.rs".into(),
        entry: "x/".into(),
        source: crate::gate::mandate_gates::PathSource::Operator,
    };
    assert_eq!(
        crate::gate::mandate::ship_decision(&c),
        Err(MandateRefusal::TouchesGates(hit.clone()))
    );
    let msg = MandateRefusal::TouchesGates(hit).message(7);
    assert!(
        msg.contains("`x/`") && msg.contains("~/.aida/config.toml"),
        "{msg}"
    );
    // The floor alone lets the same PR through to hold cover.
    c.protected = crate::gate::mandate_gates::ProtectedPaths::floor_ref();
    assert!(!matches!(
        crate::gate::mandate::ship_decision(&c),
        Err(MandateRefusal::TouchesGates(_))
    ));
}

// trace:STORY-1888.acd4bc79 | ai:claude
#[test]
fn plan_held_pr_uses_effective_set() {
    let held = || {
        vec![HeldPr {
            pr: 11,
            releasable: true,
            head: Some("d".repeat(40)),
            spec: None,
            approved_at_head: true,
            changed_paths: Some(vec!["x/y.rs".into()]),
        }]
    };
    // The tick reads the protected set from its gate load ...
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
    inputs.held_prs = held();
    inputs.gates = Ok(HumanGates {
        protected: operator_protected(),
        ..Default::default()
    });
    let mut fake = Fake::default();
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert!(fake.due.is_empty());
    assert_eq!(report.rows[0].outcome, Outcome::Gated);
    let why = report.rows[0].why.as_deref().unwrap();
    assert!(
        why.contains("`x/`") && why.contains("~/.aida/config.toml"),
        "{why}"
    );
    // ... so without the operator entry the same PR is a ship.
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
    inputs.held_prs = held();
    let mut fake = Fake::default();
    run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert_eq!(fake.due.len(), 1);
    assert_eq!(fake.due[0].1.command, "aida pr ship 11");
}

// ── T7.12: principal under the scheduler ────────────────────────────────────

// trace:STORY-1700.ac8eabb8 | ai:claude
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

// trace:STORY-1700.ace038d6 | ai:claude
#[test]
fn unknown_scope_string_fails_closed() {
    // relay-restart became a real scope in STORY-1706. trace:STORY-1706 | ai:claude
    for bogus in ["relay-kill", "bogus"] {
        let dir = tempfile::tempdir().unwrap();
        let mut body = serde_json::to_value(drive_mandate(dir.path(), "joe")).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        body["id"] = id.clone().into();
        body["scopes"] = serde_json::json!([bogus]);
        body["issued_at"] = (t0() + Duration::minutes(1)).to_rfc3339().into();
        write_record(dir.path(), &body);
        let (valid, warnings) = crate::gate::mandate::load_valid(dir.path(), "joe", now_clock());
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

// trace:STORY-1700.ac2ed84c | ai:claude
#[test]
fn unknown_human_gate_op_is_config_error() {
    use crate::gate::mandate_gates::parse_layer;
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
    let g = crate::gate::mandate_gates::union(committed, operator);
    assert_eq!(g.max_actions_per_tick, None);
    assert_eq!(g.describe(), "ship, dispatch");
}

// ── T7.14: kept from revision 1 ─────────────────────────────────────────────

// trace:STORY-1700.ace038d6 | ai:claude
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

// trace:STORY-1700.ace038d6 | ai:claude
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
    assert_eq!(old.scope, crate::gate::mandate::SCOPE_SHIP_HELD);
    assert!(old.include_supervised);
    let (valid, warnings) = crate::gate::mandate::load_valid(dir.path(), "joe", now_clock());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(valid[0].is_drive());
    assert_eq!(
        valid[0].effective_scopes(),
        Scope::ALL.into_iter().collect()
    );
}

// trace:STORY-1700.ace038d6 | ai:claude
#[test]
fn include_supervised_defaults_on_with_drive() {
    use crate::gate::mandate::drive_includes_supervised as inc;
    assert!(inc(true, false, false));
    assert!(!inc(true, false, true));
    assert!(!inc(false, false, false));
    assert!(inc(false, true, false));
    let parse = |args: &[&str]| crate::command_registry::try_parse_from(args);
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

// trace:STORY-1700.ac8eabb8 | ai:claude
#[test]
fn drive_start_and_stop_refuse_non_tty() {
    let dir = tempfile::tempdir().unwrap();
    let err = crate::gate::mandate::start_with(
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
    assert!(crate::gate::mandate::stop(dir.path(), &no_tty, "joe", now_clock()).is_err());
}

// trace:STORY-1700.Durability | ai:claude
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
    crate::gate::mandate::stop(dir.path(), &ok_human, "joe", now_clock()).unwrap();
    let (valid, _) = crate::gate::mandate::load_valid(dir.path(), "joe", now_clock());
    let stopped = crate::gate::mandate::state_at(&valid, now_clock());
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
    let (valid, warnings) = crate::gate::mandate::load_valid(dir.path(), "joe", now_clock());
    assert_eq!(valid.len(), 1);
    assert!(warnings.is_empty(), "{warnings:?}");
}

// trace:STORY-1700.Durability | ai:claude
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

// trace:STORY-1700.acb260c7 | ai:claude
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

// trace:STORY-1700.acb260c7 | ai:claude
#[test]
fn mandate_drive_active_predicate_field_type_checks() {
    use crate::schedule_predicate::parse;
    assert!(parse("mandate.drive_active == true").is_ok());
    assert!(parse("mandate.drive_active").is_ok());
    assert!(parse("mandate.drive_active && drain.lock_free").is_ok());
    assert!(parse("mandate.drive_active > 1").is_err());
    assert!(parse("mandate.drive_active == 3").is_err());
}

// trace:STORY-1700.Audit | ai:claude
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

// trace:STORY-1700.acb260c7 | ai:claude
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

// trace:STORY-1700.acb260c7 | ai:claude
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
    crate::gate::mandate::start(
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

// ── Round 2 (PR #2725 STRICT review) ────────────────────────────────────────

fn unowned(id: &str) -> UnownedSpec {
    UnownedSpec {
        id: id.into(),
        title: format!("{id} title"),
        live_claim: false,
        branch: false,
        open_pr: Some(false),
        epoch: 0,
    }
}

// Finding 1: approved unowned specs are a dispatch lane.
// trace:STORY-1700.ace038d6 | ai:claude
// trace:STORY-1700.acb260c7 | ai:claude
#[test]
fn unowned_approved_spec_routes_a_dispatch_to_the_advisor() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let mut claimed = unowned("TASK-2");
    claimed.live_claim = true;
    let mut branched = unowned("TASK-3");
    branched.branch = true;
    let mut pr = unowned("TASK-4");
    pr.open_pr = Some(true);
    let mut unknown = unowned("TASK-5");
    unknown.open_pr = None;
    let specs = vec![unowned("TASK-1"), claimed, branched, pr, unknown];
    let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
    inputs.unowned_specs = specs.clone();
    let mut fake = Fake::default();
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert_eq!(fake.due.len(), 1, "{report:?}");
    let item = &fake.due[0].1;
    assert_eq!(item.action, DriveAction::Redispatch);
    assert_eq!(item.lane, "spec:TASK-1");
    assert!(item
        .command
        .contains("aida agent new claude --spec TASK-1 --bg"));
    assert!(item.command.contains("never an in-session subagent"));
    assert_eq!(
        outcomes(&report),
        vec![
            ("spec:TASK-1".into(), Outcome::Routed),
            ("spec:TASK-5".into(), Outcome::Skipped),
        ]
    );
    // Deduplicated across ticks.
    let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
    inputs.unowned_specs = specs;
    let again = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert!(again.rows.is_empty());
    assert_eq!(fake.due.len(), 1);
}

// trace:STORY-1700.ace038d6 | ai:claude
// trace:STORY-1700.ac2ed84c | ai:claude
#[test]
fn a_dispatch_gate_or_an_operator_only_spec_keeps_the_dispatch_human() {
    // `dispatch` in [mandate.human_gates].
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
    inputs.unowned_specs = vec![unowned("TASK-1")];
    inputs.gates = Ok(HumanGates {
        ops: [GateOp::Dispatch].into_iter().collect(),
        ..Default::default()
    });
    let mut fake = Fake::default();
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert!(fake.due.is_empty());
    assert_eq!(fake.notices.len(), 1);
    assert_eq!(
        outcomes(&report),
        vec![("spec:TASK-1".into(), Outcome::Gated)]
    );
    // An operator-only or decide-mode spec, with no gates configured.
    for class in ["operator-only", "decide-mode"] {
        let fx = Fixture::new();
        drive_mandate(fx.mandates.path(), "joe");
        let always = move |s: &str| (s == "TASK-1").then(|| class.to_string());
        let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
        inputs.unowned_specs = vec![unowned("TASK-1")];
        inputs.spec_always_human = &always;
        let mut fake = Fake::default();
        let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
        assert!(fake.due.is_empty(), "{class}");
        assert_eq!(fake.notices.len(), 1, "{class}");
        assert_eq!(report.rows[0].outcome, Outcome::Gated);
        assert!(report.rows[0]
            .why
            .as_deref()
            .unwrap()
            .contains(&format!("TASK-1 is {class}")));
    }
}

#[test]
fn branch_names_match_the_spec_as_a_whole_token() {
    assert!(branch_names_spec("claude/story-17", "STORY-17"));
    assert!(branch_names_spec("origin/story-17-sketch", "STORY-17"));
    assert!(!branch_names_spec("claude/story-170", "STORY-17"));
    assert!(!branch_names_spec("claude/xstory-17", "STORY-17"));
}

// Finding 2: the mandate rework-release path takes the R1 checks too.
// trace:STORY-1700.ac8eabb8 | ai:claude
// trace:STORY-1700.ac2ed84c | ai:claude
#[test]
fn a_mandate_cannot_release_a_rework_hold_on_a_pr_touching_the_gates() {
    let tmp = tempfile::tempdir().unwrap();
    let state = MandateState::Active(drive_mandate(tmp.path(), "joe"));
    let policy = Default::default();
    let ledger = Ok(Default::default());
    let hold = crate::gate::mandate::HoldFacts::Marker {
        kind: crate::gate::merge_hold_core::HoldReasonKind::Rework,
        legacy: false,
    };
    for path in [".aida/config.toml", "aida-cli-lib/src/dispatch.rs"] {
        let paths = vec![path.to_string()];
        let c = ship_check(&state, &policy, &ledger, &hold, Some(&paths));
        assert_eq!(
            crate::gate::mandate::rework_release_decision(&c),
            Err(MandateRefusal::TouchesGates(
                crate::gate::mandate_gates::GateMatch {
                    path: path.into(),
                    entry: path.into(),
                    source: crate::gate::mandate_gates::PathSource::BuiltIn,
                }
            ))
        );
    }
    for path in [
        "aida-cli-lib/src/gate/mandate.rs",
        "aida-cli-lib/src/gate/mandate_traces.rs",
        "aida-cli-lib/src/gate/mandate_drive.rs",
    ] {
        let paths = vec![path.to_string()];
        let c = ship_check(&state, &policy, &ledger, &hold, Some(&paths));
        assert_eq!(
            crate::gate::mandate::rework_release_decision(&c),
            Err(MandateRefusal::TouchesGates(
                crate::gate::mandate_gates::GateMatch {
                    path: path.into(),
                    entry: "aida-cli-lib/src/gate/".into(),
                    source: crate::gate::mandate_gates::PathSource::BuiltIn,
                }
            ))
        );
    }
    let c = ship_check(&state, &policy, &ledger, &hold, None);
    assert_eq!(
        crate::gate::mandate::rework_release_decision(&c),
        Err(MandateRefusal::ChangedPathsUnresolved)
    );
    let paths = vec!["README.md".to_string()];
    for (ship, clear) in [(true, false), (false, true)] {
        let mut c = ship_check(&state, &policy, &ledger, &hold, Some(&paths));
        c.ship_gate = ship.then(|| "`ship` is listed".to_string());
        c.hold_clear_gate = clear.then(|| "`hold-clear` is listed".to_string());
        assert!(matches!(
            crate::gate::mandate::rework_release_decision(&c),
            Err(MandateRefusal::HumanGated { .. })
        ));
    }
}

// Finding 4: always-human specs, the sticky ledger, empty-by-default gates.
// trace:STORY-1700.ac2ed84c | ai:claude
#[test]
fn spec_always_human_reads_the_store_and_the_sticky_ledger() {
    let mut plain = aida_core::Requirement::new("plain".into(), String::new());
    plain.spec_id = Some("TASK-1".into());
    let mut decide = aida_core::Requirement::new("decide".into(), String::new());
    decide.spec_id = Some("TASK-2".into());
    decide.execution_mode = Some(aida_core::ExecutionMode::Decide);
    let mut tagged = aida_core::Requirement::new("tagged".into(), String::new());
    tagged.spec_id = Some("TASK-3".into());
    tagged.tags = ["operator-only".to_string()].into_iter().collect();
    let reqs = vec![plain, decide, tagged];
    let dir = tempfile::tempdir().unwrap();
    // TASK-1 was once operator-only; the tag is gone but the ledger is sticky.
    crate::gate::mandate::record_exclusions(
        dir.path(),
        &[("TASK-1".into(), "operator-only".into(), true)],
        t0(),
    )
    .unwrap();
    let ledger = crate::gate::mandate::read_exclusions(dir.path()).unwrap();
    let f = spec_always_human_from(&reqs, Some(&ledger));
    assert_eq!(f("TASK-1").as_deref(), Some("operator-only"));
    assert_eq!(f("task-2").as_deref(), Some("decide-mode"));
    assert_eq!(f("TASK-3").as_deref(), Some("operator-only"));
    assert_eq!(f("TASK-9"), None);
    // An unreadable ledger fails closed.
    let blind = spec_always_human_from(&reqs, None);
    assert!(blind("TASK-9").unwrap().contains("unreadable"));
}

// trace:STORY-1700.ac2ed84c | ai:claude
#[test]
fn human_gates_are_empty_by_default() {
    use crate::gate::mandate_gates::{gate_reason, parse_layer, union};
    let none = parse_layer("[schedule]\nmin_gap = \"60s\"\n", "t").unwrap();
    let g = union(none, Default::default());
    assert!(g.ops.is_empty());
    assert_eq!(g.describe(), "none");
    let g = Ok(g);
    for op in [
        GateOp::Ship,
        GateOp::SketchSignoff,
        GateOp::Dispatch,
        GateOp::HoldClear,
    ] {
        assert_eq!(gate_reason(&g, op), None);
    }
}

// Finding 5: the ledger read-backs.
// trace:STORY-1700.Audit | ai:claude
#[test]
fn mandate_status_shows_counts_and_the_last_ten_actions_newest_first() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    for i in 0..12 {
        let outcome = if i % 3 == 0 {
            Outcome::Gated
        } else {
            Outcome::Routed
        };
        let row = ActionRow::new(
            &format!("relay:r{i}"),
            &format!("run r{i}"),
            "dispatch",
            outcome,
            None,
        );
        emit_action(root.path(), "m-2", "tick", &row);
    }
    let json = ledger_json(root.path(), "m-2");
    assert_eq!(json["counts"]["gated"], 4);
    assert_eq!(json["counts"]["routed"], 8);
    let last = json["last"].as_array().unwrap();
    assert_eq!(last.len(), 10);
    assert_eq!(last[0]["subject"], "run r11");
    assert_eq!(last[9]["subject"], "run r2");
    let text = ledger_text(root.path(), "m-2").unwrap();
    assert!(
        text.contains("mandate-driven actions: 4 gated, 8 routed"),
        "{text}"
    );
    assert!(text.contains("last 10:"));
    assert!(text.find("run r11").unwrap() < text.find("run r2").unwrap());
    assert!(!text.contains("run r1 "));
    assert_eq!(ledger_text(root.path(), "none"), None);
}

// Finding 7: durability on the live paths.
// trace:STORY-1700.Durability | ai:claude
#[test]
fn session_start_hook_prints_the_mandate_line() {
    let hook = include_str!("../../../../aida-core/templates/hooks/aida-session-context.sh");
    assert!(hook.contains("aida mandate status --line"));
    assert!(hook.contains("[ -n \"$mandate_note\" ] && printf '%s\\n' \"$mandate_note\""));
}

// trace:STORY-1700.Durability | ai:claude
// trace:STORY-1700.acb260c7 | ai:claude
#[test]
fn the_notice_stamp_makes_an_advisor_live_for_thirty_minutes() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().display().to_string();
    let mut env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", Some(&home_s)),
        (crate::gate::seat_authority::GRANT_ENV, None),
    ]);
    let me = crate::current_user_id(None);
    let grant =
        crate::gate::seat_authority::test_support::mint_grant_for(root.path(), &me, "advisor", &[]);
    env.set_key(crate::gate::seat_authority::GRANT_ENV, &grant);
    let now = Utc::now();
    assert!(!advisor_live(root.path(), now), "no turn stamped yet");
    stamp_grant_turn(Some(&grant), now);
    assert!(advisor_live(root.path(), now + Duration::minutes(29)));
    assert!(!advisor_live(root.path(), now + Duration::minutes(31)));
    // A stamp for some other grant does not count.
    stamp_grant_turn(
        Some("6f1c1d6e-0000-4000-8000-0000000000ff"),
        now + Duration::minutes(40),
    );
    assert!(!advisor_live(root.path(), now + Duration::minutes(41)));
}

// trace:STORY-1700.Durability | ai:claude
#[test]
fn the_per_turn_notice_reads_the_live_mandate_and_marks_each_session_once() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().display().to_string();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", Some(&home_s)),
        (crate::gate::seat_authority::GRANT_ENV, None),
        ("AIDA_SESSION_ROLE", None),
    ]);
    let dir = crate::gate::mandate::mandate_dir().unwrap();
    let now = Utc::now();
    let m = crate::gate::mandate::start_with(
        &dir,
        &ok_human,
        &crate::current_user_id(None),
        Duration::hours(2),
        Some("fire".into()),
        true,
        true,
        now,
    )
    .unwrap();
    let line = crate::status_awaiting::mandate_notice_line(now, Some("s1")).unwrap();
    assert!(line.starts_with("MANDATE drive · "), "{line}");
    assert!(line.contains(" · fire · gates: "), "{line}");
    crate::gate::mandate::stop(&dir, &ok_human, &crate::current_user_id(None), now).unwrap();
    // Two role-less sessions each see the stopped line once.
    for session in ["s1", "s2"] {
        let first = crate::status_awaiting::mandate_notice_line(now, Some(session)).unwrap();
        assert!(first.contains(&format!("MANDATE {} was stopped", m.id)));
        assert_eq!(
            crate::status_awaiting::mandate_notice_line(now, Some(session)),
            None
        );
    }
}

// Finding 8.
// trace:STORY-1700.Durability | ai:claude
#[test]
fn seen_markers_key_on_the_grant_then_the_session_then_the_role() {
    use crate::status_awaiting::mandate_seen_key as key;
    let g = "6f1c1d6e-0000-4000-8000-0000000000aa";
    assert_eq!(
        key(Some(g), Some("s"), Some("advisor")),
        format!("grant-{g}")
    );
    assert_eq!(
        key(Some("not-a-uuid"), Some("s"), Some("advisor")),
        "session-s"
    );
    assert_eq!(key(None, None, Some("advisor")), "advisor");
    assert_eq!(key(None, Some(" "), None), "default");
}

// Finding 9.
// trace:STORY-1700.acb260c7 | ai:claude
#[test]
fn a_failed_absence_notice_is_retried_next_tick() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let mut fake = Fake {
        fail_notices: 1,
        ..Default::default()
    };
    let tick = |fake: &mut Fake| {
        let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
        inputs.advisor_live = false;
        run_tick(&inputs, &TickHooks::default(), fake)
    };
    let first = tick(&mut fake);
    assert_eq!(first.rows[0].outcome, Outcome::Refused);
    assert!(fake.notices.is_empty());
    let second = tick(&mut fake);
    assert_eq!(second.rows[0].outcome, Outcome::Done);
    assert_eq!(fake.notices.len(), 1);
    tick(&mut fake);
    assert_eq!(fake.notices.len(), 1, "then once per episode");
}

// Finding 10.
// trace:STORY-1700.ace038d6 | ai:claude
#[test]
fn only_a_store_spec_awaiting_signoff_makes_a_blocked_relay_a_sketch_signoff() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("sessions")).unwrap();
    let handoff = tmp.path().join("sessions/h.md");
    let l = lane("r", LaneClass::Blocked, tmp.path(), &[("state", "blocked")]);
    std::fs::write(
        &handoff,
        "BLOCKED_ON_HUMAN: does PR-12 still follow ADR-77?\n",
    )
    .unwrap();
    let (action, _, _, spec) = resolving_action(&l, &fixture_signoff);
    assert_eq!((action, spec.is_empty()), (DriveAction::Answer, true));
    std::fs::write(
        &handoff,
        "BLOCKED_ON_HUMAN: per ADR-77 and PR-12, sign off the STORY-9 sketch\n",
    )
    .unwrap();
    let (action, _, _, spec) = resolving_action(&l, &story_9_awaits);
    assert_eq!(action, DriveAction::SketchSignoff);
    assert_eq!(spec.first().map(String::as_str), Some("STORY-9"));

    // awaits_signoff fails closed: an open spec awaits one unless a ruling
    // is newer than its every other comment, whatever the request says.
    let mut req = aida_core::Requirement::new("s".into(), String::new());
    req.spec_id = Some("STORY-9".into());
    let comment = |text: &str, at: DateTime<Utc>| {
        let mut c = aida_core::Comment::new("joe".into(), text.into());
        c.created_at = at;
        c
    };
    assert!(awaits_signoff(&req), "no recorded signoff at all");
    req.comments.push(comment("please look at my plan", t0()));
    assert!(awaits_signoff(&req));
    req.comments.push(comment(
        "ADVISOR SIGNOFF: approved",
        t0() + Duration::minutes(5),
    ));
    assert!(!awaits_signoff(&req));
    req.comments.push(comment(
        "SKETCH REVISION 2 READY for advisor signoff",
        t0() + Duration::minutes(9),
    ));
    assert!(awaits_signoff(&req));
    let reqs = vec![req];
    let f = spec_signoff_from(&reqs);
    assert_eq!(f("story-9"), Some(true));
    assert_eq!(f("PR-12"), None);
}

// Finding 11.
// trace:STORY-1700.Durability | ai:claude
#[test]
fn the_gate_description_is_cached_on_the_origin_main_commit() {
    let repo = repo_with_committed_gates("[\"dispatch\"]");
    let oid = crate::mandate_gates::origin_main_oid(repo.path()).unwrap();
    let out = crate::test_git::git_command()
        .args(["rev-parse", "refs/remotes/origin/main"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert_eq!(oid, String::from_utf8_lossy(&out.stdout).trim());
    assert_eq!(
        crate::mandate_gates::describe(Some(repo.path())),
        "dispatch"
    );
    let cache = repo.path().join(".aida/mandate-gates-cache.json");
    assert!(cache.exists());
    // A forged cache entry under the current key is what describe returns
    // (no git run), and a new origin/main commit invalidates it.
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&cache).unwrap()).unwrap();
    v["describe"] = "cached".into();
    std::fs::write(&cache, v.to_string()).unwrap();
    assert_eq!(crate::mandate_gates::describe(Some(repo.path())), "cached");
    std::fs::write(
        repo.path().join(".aida/config.toml"),
        "[mandate.human_gates]\nops = [\"ship\"]\n",
    )
    .unwrap();
    git(repo.path(), &["commit", "-qam", "ship gate"]);
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    assert_eq!(crate::mandate_gates::describe(Some(repo.path())), "ship");
}

// trace:STORY-1700 | ai:claude
#[test]
fn word_labelled_criteria_can_be_traced() {
    assert_eq!(
        crate::parse_trace_id_token("// trace:STORY-1700.Durability | ai:claude").as_deref(),
        Some("STORY-1700.DURABILITY")
    );
    assert_eq!(
        crate::parse_trace_id_token("// trace:STORY-1700.Audit | ai:claude").as_deref(),
        Some("STORY-1700.AUDIT")
    );
    // Existing shapes are unchanged; a lowercase or all-caps word is not a label.
    assert_eq!(
        crate::parse_trace_id_token("trace:STORY-1700.ac2ed84c").as_deref(),
        Some("STORY-1700.AC2ED84C")
    );
    assert_eq!(
        crate::parse_trace_id_token("trace:STORY-1.rs").as_deref(),
        Some("STORY-1")
    );
    assert_eq!(
        crate::parse_trace_id_token("trace:STORY-1.TXT").as_deref(),
        Some("STORY-1")
    );
}

// ── Round 3 (PR #2725 scoped rework) ────────────────────────────────────────

fn blocked_lane_asking(fx: &Fixture, run: &str, question: &str) -> Lane {
    let d = fx.run_dir(run);
    std::fs::write(
        d.join("sessions/h.md"),
        format!("BLOCKED_ON_HUMAN: {question}\n"),
    )
    .unwrap();
    lane(
        run,
        LaneClass::Blocked,
        &d,
        &[("state", "blocked"), ("exit", "3")],
    )
}

// F1: an unreadable store withholds every gated seat action.
// trace:STORY-1700.ac2ed84c | ai:claude
// trace:STORY-1700.acb260c7 | ai:claude
#[test]
fn an_unreadable_store_routes_nothing_gated() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let lanes = || {
        vec![
            blocked_lane_asking(&fx, "ask", "which port should the server use?"),
            lane(
                "crash",
                LaneClass::Crashed,
                &fx.run_dir("crash"),
                &[("state", "running")],
            ),
            lane(
                "api",
                LaneClass::TerminalApi,
                &fx.run_dir("api"),
                &[("exit", "7")],
            ),
        ]
    };
    let held = || {
        vec![HeldPr {
            pr: 3,
            releasable: true,
            head: Some("e".repeat(40)),
            spec: None,
            approved_at_head: true,
            changed_paths: Some(vec!["README.md".into()]),
        }]
    };
    let tick = |fake: &mut Fake| {
        let mut inputs = fx.inputs(&now_clock, lanes(), "joe");
        inputs.held_prs = held();
        inputs.unowned_specs = vec![unowned("TASK-1")];
        inputs.store_readable = false;
        run_tick(&inputs, &TickHooks::default(), fake)
    };
    let mut fake = Fake::default();
    let report = tick(&mut fake);
    assert!(
        fake.due.is_empty(),
        "no answer, dispatch or ship is routed: {report:?}"
    );
    let store_rows: Vec<_> = report.rows.iter().filter(|r| r.lane == "store").collect();
    assert_eq!(store_rows.len(), 1);
    assert_eq!(store_rows[0].outcome, Outcome::Refused);
    assert!(store_rows[0]
        .why
        .as_deref()
        .unwrap()
        .contains("store unreadable: 4 gated seat action(s) withheld"));
    // The ungated credits notice still reaches the operator, plus the
    // store escalation.
    assert_eq!(fake.notices.len(), 2);
    // Deduplicated: the next tick ledgers no second refusal.
    let again = tick(&mut fake);
    assert!(again.rows.iter().all(|r| r.lane != "store"));
    assert!(fake.due.is_empty());
}

// F2: a relay answer is gated like a signoff, and a signoff request needs no
// keyword to be one.
// trace:STORY-1700.ac2ed84c | ai:claude
// trace:STORY-1700.ace038d6 | ai:claude
#[test]
fn a_blocked_relay_answer_is_gated_and_a_plain_signoff_request_is_a_signoff() {
    assert_eq!(DriveAction::Answer.gate_op(), Some(GateOp::SketchSignoff));
    assert_eq!(DriveAction::Answer.scope(), Some(Scope::SketchSignoff));
    let gate = || {
        Ok(HumanGates {
            ops: [GateOp::SketchSignoff].into_iter().collect(),
            ..Default::default()
        })
    };
    // A plain question (no spec named) under a sketch-signoff gate.
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let mut inputs = fx.inputs(
        &now_clock,
        vec![blocked_lane_asking(
            &fx,
            "q",
            "which port should the server use?",
        )],
        "joe",
    );
    inputs.gates = gate();
    let mut fake = Fake::default();
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert!(fake.due.is_empty());
    assert_eq!(outcomes(&report), vec![("relay:q".into(), Outcome::Gated)]);

    // A signoff request whose spec never says SKETCH is still a signoff.
    let mut req = aida_core::Requirement::new("s".into(), String::new());
    req.spec_id = Some("STORY-9".into());
    req.comments.push(aida_core::Comment::new(
        "joe".into(),
        "here is my plan".into(),
    ));
    let reqs = vec![req];
    let signoff = spec_signoff_from(&reqs);
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let l = blocked_lane_asking(&fx, "s", "please approve the STORY-9 plan");
    let (action, _, _, spec) = resolving_action(&l, &signoff);
    assert_eq!(action, DriveAction::SketchSignoff);
    assert_eq!(spec.first().map(String::as_str), Some("STORY-9"));
    let mut inputs = fx.inputs(&now_clock, vec![l], "joe");
    inputs.spec_signoff = &signoff;
    inputs.gates = gate();
    let mut fake = Fake::default();
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert!(fake.due.is_empty());
    assert_eq!(outcomes(&report), vec![("relay:s".into(), Outcome::Gated)]);

    // A relay naming an operator-only spec takes that floor, even as an Answer.
    let always = |s: &str| (s == "STORY-9").then(|| "operator-only".to_string());
    let signed = |s: &str| (s == "STORY-9").then_some(false);
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let l = blocked_lane_asking(&fx, "o", "does STORY-9 need a migration?");
    let mut inputs = fx.inputs(&now_clock, vec![l], "joe");
    inputs.spec_signoff = &signed;
    inputs.spec_always_human = &always;
    let mut fake = Fake::default();
    let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
    assert!(fake.due.is_empty());
    assert_eq!(report.rows[0].outcome, Outcome::Gated);
    assert!(report.rows[0]
        .why
        .as_deref()
        .unwrap()
        .contains("STORY-9 is operator-only"));
}

// F10: a future-dated turn stamp is not a live advisor.
// trace:STORY-1700.acb260c7 | ai:claude
#[test]
fn a_future_dated_turn_stamp_is_not_a_live_advisor() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().display().to_string();
    let mut env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", Some(&home_s)),
        (crate::gate::seat_authority::GRANT_ENV, None),
    ]);
    let me = crate::current_user_id(None);
    let grant =
        crate::gate::seat_authority::test_support::mint_grant_for(root.path(), &me, "advisor", &[]);
    env.set_key(crate::gate::seat_authority::GRANT_ENV, &grant);
    let now = Utc::now();
    stamp_grant_turn(Some(&grant), now + Duration::minutes(10));
    assert!(
        !advisor_live(root.path(), now),
        "a stamp from the future is clock skew"
    );
    assert!(advisor_live(root.path(), now + Duration::minutes(11)));
}

// ── Round 3 follow-up (R3-1..R3-3) ──────────────────────────────────────────

// R3-1: every store spec on the BLOCKED_ON_HUMAN line takes the floor.
// trace:STORY-1700.ac2ed84c | ai:claude
#[test]
fn every_spec_a_blocked_relay_names_takes_the_operator_only_floor() {
    // `ADR-77` and `STORY-9` are both store specs; only STORY-9 is
    // operator-only, and it is the second one named.
    let known = |s: &str| matches!(s, "ADR-77" | "STORY-9" | "STORY-8").then_some(false);
    let always = |s: &str| (s == "STORY-9").then(|| "operator-only".to_string());
    for question in [
        "per ADR-77, approve the STORY-9 sketch",
        "does STORY-8 still block STORY-9?",
    ] {
        let fx = Fixture::new();
        drive_mandate(fx.mandates.path(), "joe");
        let l = blocked_lane_asking(&fx, "m", question);
        let (_, _, _, specs) = resolving_action(&l, &known);
        assert!(specs.contains(&"STORY-9".to_string()), "{specs:?}");
        assert!(specs.len() == 2, "{specs:?}");
        let mut inputs = fx.inputs(&now_clock, vec![l], "joe");
        inputs.spec_signoff = &known;
        inputs.spec_always_human = &always;
        let mut fake = Fake::default();
        let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
        assert!(fake.due.is_empty(), "{question}");
        assert_eq!(report.rows[0].outcome, Outcome::Gated, "{question}");
        assert!(report.rows[0]
            .why
            .as_deref()
            .unwrap()
            .contains("STORY-9 is operator-only"));
    }
    // Any one spec awaiting a signoff makes the lane a signoff request.
    let mixed = |s: &str| match s {
        "ADR-77" => Some(false),
        "STORY-9" => Some(true),
        _ => None,
    };
    let fx = Fixture::new();
    let l = blocked_lane_asking(&fx, "w", "per ADR-77, approve the STORY-9 sketch");
    let (action, _, _, specs) = resolving_action(&l, &mixed);
    assert_eq!(action, DriveAction::SketchSignoff);
    assert_eq!(specs, vec!["STORY-9".to_string(), "ADR-77".to_string()]);
}

// R3-2: re-dispatching or fixing a run carries the run's spec.
// trace:STORY-1700.ac2ed84c | ai:claude
// trace:STORY-1700.ace038d6 | ai:claude
#[test]
fn a_redispatch_or_setup_lane_carries_the_runs_spec_and_fails_closed_without_one() {
    let always = |s: &str| (s == "TASK-9").then(|| "operator-only".to_string());
    for class in [
        LaneClass::Crashed,
        LaneClass::SessionCap,
        LaneClass::NoSlot,
        LaneClass::SetupError,
        LaneClass::UsageBin,
    ] {
        // The run record names an operator-only spec: gated by that floor.
        let fx = Fixture::new();
        drive_mandate(fx.mandates.path(), "joe");
        let l = lane("t", class, &fx.run_dir("t"), &[("ticket", "TASK-9")]);
        // TASK-1925: the store knows TASK-9. trace:TASK-1955 | ai:claude
        let knows_task_9 = |s: &str| (s == "TASK-9").then_some(false);
        let LaneDecision::Act(p) = plan_relay_lane(&l, &knows_task_9) else {
            panic!("{class:?} is a lane")
        };
        assert_eq!(p.spec.as_deref(), Some("TASK-9"), "{class:?}");
        let mut inputs = fx.inputs(&now_clock, vec![l], "joe");
        inputs.spec_always_human = &always;
        inputs.spec_signoff = &knows_task_9;
        let mut fake = Fake::default();
        let report = run_tick(&inputs, &TickHooks::default(), &mut fake);
        assert!(fake.due.is_empty(), "{class:?}");
        assert_eq!(report.rows[0].outcome, Outcome::Gated, "{class:?}");
        assert!(report.rows[0]
            .why
            .as_deref()
            .unwrap()
            .contains("TASK-9 is operator-only"));

        // No spec in the run record: gated, never routed blind.
        let fx = Fixture::new();
        drive_mandate(fx.mandates.path(), "joe");
        let l = lane("u", class, &fx.run_dir("u"), &[("ticket", "")]);
        assert_eq!(run_spec(&l, &|_| true), None);
        let mut fake = Fake::default();
        let report = run_tick(
            &fx.inputs(&now_clock, vec![l], "joe"),
            &TickHooks::default(),
            &mut fake,
        );
        assert!(fake.due.is_empty(), "{class:?}");
        assert_eq!(report.rows[0].outcome, Outcome::Gated, "{class:?}");
        assert!(report.rows[0]
            .why
            .as_deref()
            .unwrap()
            .contains("could not be determined"));
    }
}

/// Preview gate inputs with no human gate and no always-human spec.
fn ungated(store_readable: bool) -> PreviewGates<'static> {
    PreviewGates {
        store_readable,
        gates: Ok(HumanGates::default()),
        spec_always_human: &|_| None,
    }
}

/// TASK-1933: the preview applies the tick's `[mandate.human_gates]` op gate
/// and the operator-only/sticky floor, in the human and the `--json` form, so
/// it never shows routed (`held: null`) a lane the tick would gate.
// trace:TASK-1955 | ai:claude (TASK-1933)
#[test]
fn the_preview_shows_human_gates_and_the_operator_only_floor_as_the_tick_does() {
    let decisions = vec![plan_unowned_spec(&unowned("STORY-9"))];
    assert_eq!(
        preview_json(&decisions, &ungated(true))["lanes"][0]["held"],
        serde_json::Value::Null
    );

    let op_gated = PreviewGates {
        store_readable: true,
        gates: Ok(HumanGates {
            ops: [GateOp::Dispatch].into_iter().collect(),
            ..Default::default()
        }),
        spec_always_human: &|_| None,
    };
    let held = preview_json(&decisions, &op_gated)["lanes"][0]["held"].clone();
    assert!(
        held.as_str().is_some_and(|h| h.starts_with("gated: ")),
        "{held}"
    );
    assert!(preview_lines(&decisions, &op_gated)[0].contains("(gated: "));

    let operator_only = PreviewGates {
        store_readable: true,
        gates: Ok(HumanGates::default()),
        spec_always_human: &|s| (s == "STORY-9").then(|| "operator-only".to_string()),
    };
    assert_eq!(
        preview_json(&decisions, &operator_only)["lanes"][0]["held"],
        "gated: STORY-9 is operator-only"
    );
}

// `aida fleet drive --json` (preview) emits JSON even with no lane to drive,
// and carries the same withheld status the human preview shows.
// trace:STORY-1700 | ai:claude
#[test]
fn the_json_preview_is_json_with_no_lanes_and_mirrors_withheld_routes() {
    let empty = preview_json(&[], &ungated(true));
    assert_eq!(empty["store_readable"], true);
    assert_eq!(empty["lanes"], serde_json::json!([]));

    let fx = Fixture::new();
    let decisions = vec![plan_relay_lane(
        &lane("c", LaneClass::Crashed, &fx.run_dir("c"), &[]),
        &fixture_signoff,
    )];
    let readable = preview_json(&decisions, &ungated(true));
    assert_eq!(readable["lanes"][0]["held"], serde_json::Value::Null);
    let blind = preview_json(&decisions, &ungated(false));
    assert_eq!(blind["store_readable"], false);
    let lane = &blind["lanes"][0];
    assert!(lane["subject"].as_str().unwrap().contains("relay run c"));
    assert_eq!(
        lane["held"], "withheld: the requirement store is unreadable",
        "{blind}"
    );
}

// R3-3: the preview mirrors an unreadable store.
// trace:STORY-1700.ac2ed84c | ai:claude
#[test]
fn the_preview_warns_and_shows_routes_withheld_when_the_store_is_unreadable() {
    let fx = Fixture::new();
    let decisions = vec![
        plan_relay_lane(
            &lane("c", LaneClass::Crashed, &fx.run_dir("c"), &[]),
            &fixture_signoff,
        ),
        plan_relay_lane(
            &lane("a", LaneClass::TerminalApi, &fx.run_dir("a"), &[]),
            &fixture_signoff,
        ),
    ];
    let readable = preview_lines(&decisions, &ungated(true));
    assert!(readable
        .iter()
        .all(|l| !l.contains("withheld") && !l.starts_with("warning")));
    let blind = preview_lines(&decisions, &ungated(false));
    assert!(blind[0].starts_with("warning: the requirement store is unreadable"));
    let crash = blind.iter().find(|l| l.contains("relay run c")).unwrap();
    assert!(
        crash.contains("withheld: the requirement store is unreadable"),
        "{crash}"
    );
    let api = blind.iter().find(|l| l.contains("relay run a")).unwrap();
    assert!(
        !api.contains("withheld"),
        "an ungated operator notice is not withheld: {api}"
    );
    assert_eq!(
        preview_lines(&[], &ungated(false)).len(),
        1,
        "the warning shows even with no lanes"
    );
}

// ── TASK-1915: STORY-1700 hardening follow-ups (PR #2725 F3-F8) ─────────────

fn stop_before(fx: &Fixture, at: usize, advisor_live: bool) -> (TickReport, Fake) {
    drive_mandate(fx.mandates.path(), "joe");
    let dir = fx.mandates.path().to_path_buf();
    let hook = move |n: usize| {
        if n == at {
            crate::gate::mandate::stop(&dir, &ok_human, "joe", now_clock()).unwrap();
        }
    };
    let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
    inputs.advisor_live = advisor_live;
    let mut fake = Fake::default();
    let report = run_tick(
        &inputs,
        &TickHooks {
            before_action: Some(&hook),
            ..Default::default()
        },
        &mut fake,
    );
    (report, fake)
}

// F3: a failed recheck before the reap or the escalation is a ledgered
// refusal, not a silent end. trace:TASK-1915 | ai:claude
#[test]
fn a_recheck_failure_before_the_reap_or_the_escalation_is_ledgered() {
    let (report, fake) = stop_before(&Fixture::new(), 0, true);
    assert_eq!(outcomes(&report), vec![("leases".into(), Outcome::Refused)]);
    assert_eq!(
        report.rows[0].why.as_deref(),
        Some("mandate stopped mid-tick")
    );
    assert_eq!(fake.rows.len(), 1);

    let (report, fake) = stop_before(&Fixture::new(), 1, false);
    assert_eq!(
        outcomes(&report),
        vec![("advisor".into(), Outcome::Refused)]
    );
    assert_eq!(report.rows[0].action, "escalate");
    assert!(fake.notices.is_empty());
}

// F4: every reap outcome is a row — teardown, branch deletion, GC, skips
// and failures — not one "leases released" count. trace:TASK-1915 | ai:claude
#[test]
fn every_reap_outcome_is_ledgered() {
    let rows = reap_rows(crate::session_reap::QuietReap {
        agent_gc: Ok(2),
        leases: vec![
            (
                "s1".into(),
                "reaped — worktree removed, lease released, branch `x` deleted".into(),
            ),
            (
                "s2".into(),
                "skipped — worktree became dirty since the scan".into(),
            ),
            ("s3".into(), "failed — could not remove worktree /w".into()),
        ],
    });
    let kinds: Vec<_> = rows
        .iter()
        .map(|r| (r.subject.as_str(), r.outcome))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("agent registry", Outcome::Done),
            ("session s1", Outcome::Done),
            ("session s2", Outcome::Skipped),
            ("session s3", Outcome::Refused),
        ]
    );
    assert!(rows[1].why.contains("branch `x` deleted"));
    let failed = reap_rows(crate::session_reap::QuietReap {
        agent_gc: Err("registry locked".into()),
        leases: Vec::new(),
    });
    assert_eq!(failed[0].outcome, Outcome::Refused);
    assert!(failed[0].why.contains("registry locked"));

    // The tick ledgers each; a reap skipped for the same reason next tick
    // is not ledgered again (F7).
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let mut fake = Fake {
        reap: rows.clone(),
        ..Default::default()
    };
    let report = run_tick(
        &fx.inputs(&now_clock, Vec::new(), "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    assert_eq!(report.rows.len(), 4, "{report:?}");
    assert!(report
        .rows
        .iter()
        .all(|r| r.lane == "leases" && r.action == "reap"));
    fake.reap = rows
        .into_iter()
        .filter(|r| r.outcome == Outcome::Skipped)
        .collect();
    let again = run_tick(
        &fx.inputs(&now_clock, Vec::new(), "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    assert!(again.rows.is_empty(), "{again:?}");
}

// F5: a mail failure retries the mail only; the due event is written once.
// trace:TASK-1915 | ai:claude
#[test]
fn a_mail_failure_retry_does_not_duplicate_the_due_event() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    let LaneDecision::Act(item) = plan_unowned_spec(&unowned("TASK-1")) else {
        panic!("an unowned spec is a dispatch");
    };
    let fail = |_: &Path, _: &aida_core::mailbox::Message| -> Result<()> {
        anyhow::bail!("mailbox is read-only")
    };
    let mut recorded = false;
    assert!(emit_due_with(root.path(), "m-1", &item, &mut recorded, &fail).is_err());
    assert!(recorded, "the event was written before the mail failed");
    let sent = Cell::new(0usize);
    let ok = |_: &Path, _: &aida_core::mailbox::Message| -> Result<()> {
        sent.set(sent.get() + 1);
        Ok(())
    };
    emit_due_with(root.path(), "m-1", &item, &mut recorded, &ok).unwrap();
    let due = crate::events::read_all(root.path())
        .into_iter()
        .filter(|ev| {
            matches!(&ev.kind, crate::events::EventKind::MandateDriveDue { key, .. }
                if *key == item.key())
        })
        .count();
    assert_eq!(due, 1);
    assert_eq!(sent.get(), 1);
}

// F6: a corrupt state file fails closed instead of resetting dedup.
// trace:TASK-1915 | ai:claude
#[test]
fn a_corrupt_drive_state_fails_closed() {
    let fx = Fixture::new();
    assert_eq!(load_state(fx.root.path()).unwrap(), DriveState::default());
    drive_mandate(fx.mandates.path(), "joe");
    let path = state_path(fx.root.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{not json").unwrap();
    assert!(load_state(fx.root.path()).is_err());
    let mut fake = Fake::default();
    let report = run_tick(
        &fx.inputs(&now_clock, three_lanes(&fx), "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    assert!(fake.due.is_empty());
    assert_eq!(report.rows.len(), 1);
    assert_eq!(report.rows[0].action, "state-check");
    assert_eq!(report.rows[0].outcome, Outcome::Refused);
    assert_eq!(fake.rows.len(), 1, "the refusal is ledgered");
    assert!(report.idle.is_some());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
    assert_eq!(fake.notices.len(), 1, "the operator is told once");
    // The same corrupt state is not re-ledgered or re-notified every tick...
    let again = run_tick(
        &fx.inputs(&now_clock, three_lanes(&fx), "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    assert!(again.rows.is_empty(), "{again:?}");
    assert!(again.idle.is_some(), "it still refuses");
    assert_eq!((fake.rows.len(), fake.notices.len()), (1, 1));
    // ...but a differently corrupt one is.
    std::fs::write(&path, "[still not a state]").unwrap();
    let changed = run_tick(
        &fx.inputs(&now_clock, three_lanes(&fx), "joe"),
        &TickHooks::default(),
        &mut fake,
    );
    assert_eq!(changed.rows.len(), 1);
    assert!(fake.due.is_empty());

    // With no own active mandate, nothing is ledgered under an empty id;
    // the R4 mismatch (another principal's record) is still told, once.
    // trace:TASK-1955 | ai:claude
    let other = Fixture::new();
    drive_mandate(other.mandates.path(), "a");
    let path = state_path(other.root.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{not json").unwrap();
    let mut fake = Fake::default();
    let report = run_tick(
        &other.inputs(&now_clock, Vec::new(), "b"),
        &TickHooks::default(),
        &mut fake,
    );
    assert!(report.idle.is_some());
    assert_eq!(fake.rows.len(), 1, "{:?}", fake.rows);
    assert!(!fake.rows[0].0.is_empty());
    assert_eq!(
        fake.rows[0].1.why.as_deref(),
        Some("principal mismatch: record=a tick=b")
    );
    assert_eq!(fake.notices.len(), 1);
    let again = run_tick(
        &other.inputs(&now_clock, Vec::new(), "b"),
        &TickHooks::default(),
        &mut fake,
    );
    assert!(again.rows.is_empty(), "{again:?}");
    assert_eq!((fake.rows.len(), fake.notices.len()), (1, 1));
}

// F6: the mismatch notice is marked sent only once it is sent.
// trace:TASK-1915 | ai:claude
#[test]
fn a_failed_mismatch_notice_is_retried() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "a");
    let tick = |fake: &mut Fake| {
        run_tick(
            &fx.inputs(&now_clock, Vec::new(), "b"),
            &TickHooks::default(),
            fake,
        )
    };
    let mut fake = Fake {
        fail_notices: 1,
        ..Default::default()
    };
    let first = tick(&mut fake);
    assert!(fake.notices.is_empty());
    assert!(first.rows[0]
        .why
        .as_deref()
        .unwrap()
        .contains("notify failed"));
    let second = tick(&mut fake);
    assert_eq!(fake.notices.len(), 1, "the failed notice is retried");
    assert_eq!(second.rows.len(), 1);
    assert!(tick(&mut fake).rows.is_empty(), "then once per mandate");
}

// F7: a skip row is ledgered once per (item, reason), not every tick.
// trace:TASK-1915 | ai:claude
#[test]
fn skip_rows_are_ledgered_once_per_item_and_reason() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let tick = |fake: &mut Fake| {
        let mut inputs = fx.inputs(&now_clock, three_lanes(&fx), "joe");
        inputs.gates = Ok(HumanGates {
            max_actions_per_tick: Some(1),
            ..Default::default()
        });
        run_tick(&inputs, &TickHooks::default(), fake)
    };
    let mut fake = Fake::default();
    let first = tick(&mut fake);
    assert_eq!(
        outcomes(&first),
        vec![
            ("relay:a".into(), Outcome::Routed),
            ("relay:b".into(), Outcome::Skipped),
            ("relay:c".into(), Outcome::Skipped),
        ]
    );
    // b acts now; c is skipped for the same budget reason: no second row.
    let second = tick(&mut fake);
    assert_eq!(outcomes(&second), vec![("relay:b".into(), Outcome::Routed)]);
}

// F8: a spec whose fixer took it and died, or never showed up, is routed
// again under a new dispatch epoch. trace:TASK-1915 | ai:claude
#[test]
fn a_dead_fixers_spec_is_routed_again() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let tick = |spec: UnownedSpec, clock: &dyn Fn() -> DateTime<Utc>, fake: &mut Fake| {
        let mut inputs = fx.inputs(clock, Vec::new(), "joe");
        inputs.unowned_specs = vec![spec];
        run_tick(&inputs, &TickHooks::default(), fake)
    };
    let mut fake = Fake::default();
    tick(unowned("TASK-1"), &now_clock, &mut fake);
    assert_eq!(fake.due.len(), 1);
    let mut claimed = unowned("TASK-1");
    claimed.live_claim = true;
    assert!(tick(claimed, &now_clock, &mut fake).rows.is_empty());
    // The fixer is gone: the spec is unowned again, a new item.
    tick(unowned("TASK-1"), &now_clock, &mut fake);
    assert_eq!(fake.due.len(), 2);
    assert_eq!(fake.due[1].1.head, "approved#1");
    // Not again while the new epoch is young...
    let soon = || now_clock() + Duration::minutes(30);
    assert!(tick(unowned("TASK-1"), &soon, &mut fake).rows.is_empty());
    // ...but a dispatch nobody took up within the window is re-routed.
    let late = || now_clock() + Duration::hours(2) + Duration::minutes(1);
    tick(unowned("TASK-1"), &late, &mut fake);
    assert_eq!(fake.due.len(), 3);
    assert_eq!(fake.due[2].1.head, "approved#2");
}

// F8: a spec in a fresh work-queue entry or a live drain's spec set is
// owned; a stale queue entry is not.
// trace:TASK-1915 | ai:claude
#[test]
fn queue_entries_and_a_live_drain_own_their_specs() {
    let root = tempfile::tempdir().unwrap();
    let mut queued = aida_core::Requirement::new("queued".into(), String::new());
    queued.spec_id = Some("TASK-1".into());
    let mut free = aida_core::Requirement::new("free".into(), String::new());
    free.spec_id = Some("TASK-3".into());
    let queues = root.path().join(".aida-store/registry/queues");
    std::fs::create_dir_all(&queues).unwrap();
    let entry = |id: uuid::Uuid, added_at: DateTime<Utc>| aida_core::QueueEntry {
        user_id: "joe".into(),
        requirement_id: id,
        position: 0,
        added_by: "joe".into(),
        note: None,
        added_at,
        for_role: Some("implementer".into()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    std::fs::write(
        queues.join("joe.yaml"),
        serde_yaml::to_string(&vec![entry(queued.id, now_clock())]).unwrap(),
    )
    .unwrap();
    // An abandoned queue (an entry months old) owns nothing.
    std::fs::write(
        queues.join("codex.yaml"),
        serde_yaml::to_string(&vec![entry(free.id, now_clock() - Duration::days(90))]).unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    std::fs::write(
        crate::drain_lock::drain_lock_path(root.path()),
        serde_json::json!({
            "pid": std::process::id(),
            "started_at_utc": "2026-10-21T00:00:00Z",
            "command": "drain",
            "host": "h",
            "specs": ["task-2"],
        })
        .to_string(),
    )
    .unwrap();
    let held = queue_and_drain_claims(root.path(), &[queued, free], now_clock());
    assert!(held.contains("TASK-1"), "{held:?}");
    assert!(held.contains("TASK-2"), "{held:?}");
    assert!(!held.contains("TASK-3"), "{held:?}");
}

// F6: a state that cannot be saved on the principal-mismatch path is
// reported, not swallowed. trace:TASK-1915 | ai:claude
#[test]
fn a_failed_save_on_the_mismatch_path_is_reported() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "a");
    let fail = |_: &Path, _: &DriveState| -> Result<()> { anyhow::bail!("disk full") };
    let mut fake = Fake::default();
    let report = run_tick(
        &fx.inputs(&now_clock, Vec::new(), "b"),
        &TickHooks {
            save: Some(&fail),
            ..Default::default()
        },
        &mut fake,
    );
    let idle = report.idle.as_deref().unwrap_or_default();
    assert!(idle.contains("could not save the drive state"), "{idle}");
    assert!(idle.contains("disk full"), "{idle}");
}

// F3: a failed recheck before the store check is a ledgered refusal.
// trace:TASK-1915 | ai:claude
#[test]
fn a_recheck_failure_before_the_store_check_is_ledgered() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let dir = fx.mandates.path().to_path_buf();
    // Side effects: 0 = the reap, 1 = the store check.
    let hook = move |n: usize| {
        if n == 1 {
            crate::gate::mandate::stop(&dir, &ok_human, "joe", now_clock()).unwrap();
        }
    };
    let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
    inputs.store_readable = false;
    let mut fake = Fake::default();
    let report = run_tick(
        &inputs,
        &TickHooks {
            before_action: Some(&hook),
            ..Default::default()
        },
        &mut fake,
    );
    assert_eq!(outcomes(&report), vec![("store".into(), Outcome::Refused)]);
    assert_eq!(report.rows[0].action, "store-check");
    assert_eq!(
        report.rows[0].why.as_deref(),
        Some("mandate stopped mid-tick")
    );
    assert!(fake.notices.is_empty());
}

/// TASK-1925: a relay run's spec is the first `ticket`/`spec` id the store
/// knows. A store-known, ungated spec routes per scope; an id the store does
/// not know (the relay's `ticket=` is its queue-lock path, or a stray token)
/// never counts, and the lane fails closed with RUN_SPEC_UNKNOWN.
// trace:TASK-1955 | ai:claude (TASK-1925)
#[test]
fn run_spec_keeps_only_store_known_ids() {
    let fx = Fixture::new();
    let known = |s: &str| s == "STORY-4";
    let l = lane(
        "k",
        LaneClass::Crashed,
        &fx.run_dir("k"),
        &[
            ("ticket", "/run/relay-ticket-12.lock BUG-404"),
            ("spec", "STORY-4"),
        ],
    );
    assert_eq!(run_spec(&l, &known).as_deref(), Some("STORY-4"));

    let knows_story_4 = |s: &str| (s == "STORY-4").then_some(false);
    let LaneDecision::Act(p) = plan_relay_lane(&l, &knows_story_4) else {
        panic!("a crashed lane is acted on")
    };
    assert_eq!(p.spec.as_deref(), Some("STORY-4"));
    assert_eq!(
        p.forced_gate, None,
        "a known, ungated spec is not force-gated"
    );

    let unknown = lane(
        "u",
        LaneClass::Crashed,
        &fx.run_dir("u"),
        &[("ticket", "BUG-404")],
    );
    assert_eq!(run_spec(&unknown, &known), None);
    let LaneDecision::Act(p) = plan_relay_lane(&unknown, &knows_story_4) else {
        panic!("a crashed lane is acted on")
    };
    assert_eq!(p.spec, None);
    assert_eq!(p.forced_gate.as_deref(), Some(RUN_SPEC_UNKNOWN));
}

// ── TASK-1955 slice 8: drive follow-ups from the PR-2804 review ────────────

fn tick_specs(
    fx: &Fixture,
    specs: Vec<UnownedSpec>,
    clock: &dyn Fn() -> DateTime<Utc>,
    fake: &mut Fake,
) -> TickReport {
    let mut inputs = fx.inputs(clock, Vec::new(), "joe");
    inputs.unowned_specs = specs;
    run_tick(&inputs, &TickHooks::default(), fake)
}

// A dispatch nobody takes up is re-routed after 2h, 4h and 8h, then no
// more until an owner shows up; an owner that comes and goes resets that.
// trace:TASK-1955 | ai:claude (TASK-2078) (TASK-1696)
#[test]
fn an_untaken_dispatch_backs_off_and_stops_rerouting() {
    let fx = Fixture::new();
    // The longest mandate (24h): the backoff spans most of it.
    let m = crate::gate::mandate::start_with(
        fx.mandates.path(),
        &ok_human,
        "joe",
        Duration::hours(24),
        Some("fire".into()),
        true,
        true,
        t0(),
    )
    .unwrap();
    let mut fake = Fake::default();
    let at = |h: i64, m: i64| move || now_clock() + Duration::hours(h) + Duration::minutes(m);
    tick_specs(&fx, vec![unowned("TASK-1")], &at(0, 0), &mut fake);
    assert_eq!(fake.due.len(), 1);
    // 2h after the route: re-routed (epoch 1).
    tick_specs(&fx, vec![unowned("TASK-1")], &at(2, 1), &mut fake);
    assert_eq!(fake.due.len(), 2);
    // The next window is 4h, not 2h.
    tick_specs(&fx, vec![unowned("TASK-1")], &at(4, 2), &mut fake);
    assert_eq!(fake.due.len(), 2, "the window doubled");
    tick_specs(&fx, vec![unowned("TASK-1")], &at(6, 2), &mut fake);
    assert_eq!(fake.due.len(), 3);
    tick_specs(&fx, vec![unowned("TASK-1")], &at(14, 3), &mut fake);
    assert_eq!(fake.due.len(), 4);
    // Three stale re-routes spent: nothing new for the rest of the mandate.
    for h in [17, 20, 22] {
        tick_specs(&fx, vec![unowned("TASK-1")], &at(h, 30), &mut fake);
    }
    assert_eq!(fake.due.len(), 4, "{:?}", fake.due);
    assert_eq!(fake.due[3].1.head, "approved#3");
    // An owner takes it and goes away: a new epoch at once, budget reset.
    let mut claimed = unowned("TASK-1");
    claimed.live_claim = true;
    tick_specs(&fx, vec![claimed], &at(22, 40), &mut fake);
    tick_specs(&fx, vec![unowned("TASK-1")], &at(22, 50), &mut fake);
    assert_eq!(fake.due.len(), 5);
    let d = load_state(fx.root.path()).unwrap().mandates[&m.id].dispatch["TASK-1"].clone();
    assert_eq!((d.epoch, d.stale_reroutes), (4, 0), "{d:?}");
}

// A dispatch routed before epochs existed (only its emitted key, no
// routed_at) is dated from first sight, so its epoch can close.
// trace:TASK-1955 | ai:claude (TASK-2080) (TASK-1694)
#[test]
fn a_pre_epoch_dispatch_is_dated_and_rerouted() {
    let fx = Fixture::new();
    let m = drive_mandate(fx.mandates.path(), "joe");
    let mut state = DriveState::default();
    let per = state.mandates.entry(m.id.clone()).or_default();
    per.emitted.insert("spec:TASK-1|dispatch|approved".into());
    std::fs::create_dir_all(fx.root.path().join(".aida")).unwrap();
    std::fs::write(
        state_path(fx.root.path()),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    let mut fake = Fake::default();
    tick_specs(&fx, vec![unowned("TASK-1")], &now_clock, &mut fake);
    assert!(fake.due.is_empty(), "already routed before the upgrade");
    let dated = load_state(fx.root.path()).unwrap().mandates[&m.id].dispatch["TASK-1"].clone();
    assert_eq!(dated.routed_at, Some(now_clock()));
    let late = || now_clock() + Duration::hours(2) + Duration::minutes(1);
    tick_specs(&fx, vec![unowned("TASK-1")], &late, &mut fake);
    assert_eq!(fake.due.len(), 1);
    assert_eq!(fake.due[0].1.head, "approved#1");
}

// The state stays bounded: specs no longer approved lose their epoch and
// keys, an old epoch's keys go, and so do skip/refusal keys of sessions the
// reap no longer reports. An unreadable store prunes no spec state.
// trace:TASK-1955 | ai:claude (TASK-2080) (TASK-1694)
#[test]
fn drive_state_prunes_what_no_longer_names_a_live_item() {
    let fx = Fixture::new();
    let m = drive_mandate(fx.mandates.path(), "joe");
    let mut fake = Fake {
        reap: vec![ReapRow {
            subject: "session s9".into(),
            outcome: Outcome::Skipped,
            why: "skipped — worktree dirty".into(),
        }],
        ..Default::default()
    };
    let mut claimed = unowned("TASK-2");
    claimed.live_claim = true;
    tick_specs(
        &fx,
        vec![unowned("TASK-1"), claimed.clone()],
        &now_clock,
        &mut fake,
    );
    let per = |fx: &Fixture| load_state(fx.root.path()).unwrap().mandates[&m.id].clone();
    let first = per(&fx);
    assert!(first.emitted.contains("spec:TASK-1|dispatch|approved"));
    assert!(first
        .emitted
        .iter()
        .any(|k| k.starts_with("leases|session s9|")));
    assert_eq!(first.dispatch.len(), 2);
    // TASK-1 leaves Approved; s9 is gone: an unreadable store keeps the
    // spec state, the reap's view drops the session key.
    let mut inputs = fx.inputs(&now_clock, Vec::new(), "joe");
    inputs.store_readable = false;
    run_tick(&inputs, &TickHooks::default(), &mut fake);
    let blind = per(&fx);
    assert!(blind.emitted.contains("spec:TASK-1|dispatch|approved"));
    assert!(!blind.emitted.iter().any(|k| k.starts_with("leases|")));
    assert_eq!(blind.dispatch.len(), 2);
    // A readable store without TASK-1 drops it; TASK-2 stays.
    tick_specs(&fx, vec![claimed], &now_clock, &mut fake);
    let pruned = per(&fx);
    assert!(!pruned.emitted.iter().any(|k| k.starts_with("spec:TASK-1|")));
    assert_eq!(pruned.dispatch.keys().collect::<Vec<_>>(), ["TASK-2"]);
    // An old epoch's key goes once the spec moves to a new one.
    let late = || now_clock() + Duration::hours(3);
    tick_specs(&fx, vec![unowned("TASK-2")], &now_clock, &mut fake);
    tick_specs(&fx, vec![unowned("TASK-2")], &late, &mut fake);
    let moved = per(&fx);
    assert!(moved.emitted.contains("spec:TASK-2|dispatch|approved#1"));
    assert!(!moved.emitted.contains("spec:TASK-2|dispatch|approved"));
}

// A failed mail after a written due event: the retry is told the event is
// recorded (from the drive state, no event-log scan), and success clears it.
// trace:TASK-1955 | ai:claude (TASK-2084) (TASK-1693)
#[test]
fn a_mail_retry_reads_the_recorded_event_from_the_state() {
    let fx = Fixture::new();
    let m = drive_mandate(fx.mandates.path(), "joe");
    let mut fake = Fake {
        fail_mail_for: Some("spec:TASK-1".into()),
        ..Default::default()
    };
    tick_specs(&fx, vec![unowned("TASK-1")], &now_clock, &mut fake);
    let key = "spec:TASK-1|dispatch|approved";
    let state = load_state(fx.root.path()).unwrap();
    assert!(state.mandates[&m.id].due_recorded.contains(key));
    fake.fail_mail_for = None;
    let later = || now_clock() + Duration::hours(1);
    tick_specs(&fx, vec![unowned("TASK-1")], &later, &mut fake);
    assert_eq!(fake.recorded_on_entry, [false, true]);
    assert_eq!(fake.due.len(), 1);
    let state = load_state(fx.root.path()).unwrap();
    assert!(state.mandates[&m.id].due_recorded.is_empty());
    assert!(state.mandates[&m.id].emitted.contains(key));
}

// A reap that fails the same way every tick is one ledgered row; a new
// reason is a new row.
// trace:TASK-1955 | ai:claude (TASK-1697)
#[test]
fn a_persistent_reap_failure_is_ledgered_once_per_reason() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let failing = |why: &str| {
        vec![
            ReapRow {
                subject: "session s3".into(),
                outcome: Outcome::Refused,
                why: why.into(),
            },
            ReapRow {
                subject: "agent registry".into(),
                outcome: Outcome::Refused,
                why: "agent registry GC failed: EACCES".into(),
            },
        ]
    };
    let mut fake = Fake::default();
    let mut tick = |reap| {
        fake.reap = reap;
        run_tick(
            &fx.inputs(&now_clock, Vec::new(), "joe"),
            &TickHooks::default(),
            &mut fake,
        )
    };
    assert_eq!(
        tick(failing("failed — could not remove worktree /w"))
            .rows
            .len(),
        2
    );
    assert!(tick(failing("failed — could not remove worktree /w"))
        .rows
        .is_empty());
    let changed = tick(failing("failed — could not delete branch x"));
    assert_eq!(changed.rows.len(), 1);
    assert_eq!(changed.rows[0].subject, "session s3");
}

// An unwritable refusal sidecar: the tick reports why and does not ledger
// or notify the same refusal every tick.
// trace:TASK-1955 | ai:claude
#[test]
fn an_unwritable_refusal_sidecar_is_reported_not_repeated() {
    let fx = Fixture::new();
    drive_mandate(fx.mandates.path(), "joe");
    let path = state_path(fx.root.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{not json").unwrap();
    let sidecar = fx
        .root
        .path()
        .join(".aida/mandate-drive-state.refused.json");
    std::fs::create_dir_all(sidecar.join("blocker")).unwrap();
    let mut fake = Fake::default();
    for _ in 0..3 {
        let report = run_tick(
            &fx.inputs(&now_clock, three_lanes(&fx), "joe"),
            &TickHooks::default(),
            &mut fake,
        );
        let idle = report.idle.unwrap();
        assert!(idle.contains("not ledgered or notified"), "{idle}");
        assert!(idle.contains("refused.json"), "{idle}");
    }
    assert!(fake.rows.is_empty(), "{:?}", fake.rows);
    assert!(fake.notices.is_empty());
    assert!(fake.due.is_empty());
}

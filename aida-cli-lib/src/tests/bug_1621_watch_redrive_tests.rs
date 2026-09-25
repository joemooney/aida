//! BUG-1621: `aida supervise watch --execute` re-drives through the night
//! shift's floored pipeline (`plan_redrives` → `apply_requeue` → `apply_cap`),
//! behind the same per-clone opt-in, guards, floors and fail-closed attempt
//! record, and never launches a drive or forces a claim.
//!
//! The floor tests drive `shift::redrive_pass` (the function the watch pass
//! calls through `run_redrive_pass`) with a recording exec. The real-store
//! tests drive `run_redrive_pass_with` on a tempdir store with an injected
//! config, so none reads `~/.aida`, the live store, crontab or systemd.
// trace:BUG-1621 | ai:claude

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

use aida_core::{DatabaseBackend, ExecutionMode, RequirementStatus};
use chrono::{DateTime, Duration, TimeZone, Utc};

use crate::events::{self, EventKind};
use crate::shift::{
    build_config, redrive_pass, run_redrive_pass_with, state_path, Breaker, LockView, Probes,
    ShiftConfig, ShiftExec, ShiftState, TickCtx, WaveRecord,
};
use crate::supervisor::{QueueTarget, RequeueOutcome, SuperviseDecision};

const REPO: &str = "/repo/aida";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 25, 3, 0, 0).unwrap()
}

fn cfg(local: &str) -> ShiftConfig {
    let local = toml::from_str::<toml::Value>(local).unwrap();
    build_config(None, Some(&local), REPO, "L")
}

/// Re-drive opted in on this clone. The shift itself is NOT enabled: the
/// watch path needs only the re-drive switch.
fn cfg_redrive_on() -> ShiftConfig {
    cfg(&format!("[repo.\"{REPO}\"]\nredrive = true\n"))
}

fn cfg_redrive_off() -> ShiftConfig {
    cfg(&format!("[repo.\"{REPO}\"]\nenabled = true\n"))
}

fn park(
    spec: &str,
    kind: &str,
    mode: Option<ExecutionMode>,
    mins_ago: i64,
) -> aida_core::Requirement {
    park_at(spec, kind, mode, now() - Duration::minutes(mins_ago))
}

fn park_at(
    spec: &str,
    kind: &str,
    mode: Option<ExecutionMode>,
    at: DateTime<Utc>,
) -> aida_core::Requirement {
    let mut r = aida_core::Requirement::new(spec.to_string(), "parked".to_string());
    r.spec_id = Some(spec.to_string());
    r.status = RequirementStatus::NeedsAttention;
    r.execution_mode = mode;
    r.modified_at = at;
    r.failure_reason = Some(aida_core::FailureReason {
        phase: "implementer".to_string(),
        phase_index: 1,
        kind: kind.to_string(),
        detail: "failed".to_string(),
        recovery_hint: None,
        shelved_by: None,
        shelved_at: at,
    });
    r
}

fn drain_park(spec: &str, mins_ago: i64) -> aida_core::Requirement {
    park(spec, "watchdog", Some(ExecutionMode::Drain), mins_ago)
}

fn probes(parked: Vec<aida_core::Requirement>) -> Probes {
    Probes::for_redrive(
        LockView::Free,
        None,
        false,
        "joe".to_string(),
        (parked, Ok(events::RedriveHistory::default())),
        BTreeSet::new(),
    )
}

fn ctx(dry_run: bool) -> TickCtx {
    TickCtx {
        now: now(),
        dry_run,
        optional_allowed: true,
        launch_allowed: true,
        state_error: None,
        log_path: PathBuf::new(),
        clock: None,
    }
}

/// Records every side effect the pass asks for.
#[derive(Default)]
struct Rec {
    calls: Vec<String>,
    requeued: Vec<(Vec<String>, QueueTarget)>,
    reclassified: Vec<String>,
    /// The attempt record fails from this would-re-drive index on.
    unrecorded_from: Option<usize>,
    spawns: Vec<Vec<String>>,
}

impl ShiftExec for Rec {
    fn reap(&mut self) -> usize {
        self.calls.push("reap".into());
        0
    }
    fn tag_batch(&mut self, _: &str, _: &[String]) -> anyhow::Result<()> {
        self.calls.push("tag".into());
        Ok(())
    }
    fn spawn_wave(&mut self, argv: &[String], _: &Path) -> anyhow::Result<(u32, Option<String>)> {
        self.calls.push("spawn".into());
        self.spawns.push(argv.to_vec());
        Ok((1, None))
    }
    fn save_state(&mut self, _: &ShiftState) -> anyhow::Result<()> {
        self.calls.push("save".into());
        Ok(())
    }
    fn emit(&mut self, _: EventKind) {
        self.calls.push("emit".into());
    }
    fn requeue(
        &mut self,
        decisions: &[SuperviseDecision],
        queue: &QueueTarget,
    ) -> anyhow::Result<RequeueOutcome> {
        let mut specs: Vec<String> = decisions
            .iter()
            .filter(|d| d.action == "would-re-drive")
            .map(|d| d.spec.clone())
            .collect();
        let mut out = RequeueOutcome::default();
        if let Some(i) = self.unrecorded_from.filter(|i| *i < specs.len()) {
            out.held = specs.split_off(i);
            out.unrecorded = Some(format!(
                "cannot record the re-drive attempt for {}: disk full",
                out.held[0]
            ));
        }
        if !specs.is_empty() {
            self.calls.push("requeue".into());
            self.requeued.push((specs.clone(), queue.clone()));
        }
        out.applied = specs;
        Ok(out)
    }
    fn reclassify(&mut self, d: &SuperviseDecision) -> anyhow::Result<bool> {
        self.calls.push("reclassify".into());
        self.reclassified.push(d.spec.clone());
        Ok(true)
    }
    fn notify(
        &mut self,
        _: &str,
        _: &str,
        _: &str,
        _: StdDuration,
    ) -> anyhow::Result<crate::notify::DirectDelivery> {
        self.calls.push("notify".into());
        Ok(crate::notify::DirectDelivery::Sent)
    }
}

fn run(
    cfg: &ShiftConfig,
    p: &Probes,
    state: &ShiftState,
    dry_run: bool,
) -> (crate::shift::TickReport, Rec) {
    let mut rec = Rec::default();
    let r = redrive_pass(cfg, p, state, &ctx(dry_run), &mut rec).unwrap();
    (r, rec)
}

fn action(r: &crate::shift::TickReport, spec: &str) -> String {
    r.redrive_plan
        .iter()
        .find(|d| d.spec == spec)
        .map(|d| d.action.clone())
        .unwrap_or_else(|| panic!("no decision for {spec}: {:?}", r.redrive_plan))
}

fn failing_guard(r: &crate::shift::TickReport, name: &str) -> bool {
    r.redrive_guards.iter().any(|g| g.name == name && !g.pass)
}

// ---------------------------------------------------------------------------
// The opt-in and the happy path
// ---------------------------------------------------------------------------

#[test]
fn watch_redrive_is_off_by_default_even_under_execute() {
    // ADR-26 fork C: `watch --execute` is run by the oversight seat (an
    // agent) and on an `--interval` loop, so it is an unattended caller and
    // needs the same per-clone opt-in as the tick. Enabling the shift alone
    // does not turn re-drive on.
    let p = probes(vec![drain_park("TASK-9", 60)]);
    let (r, rec) = run(&cfg_redrive_off(), &p, &ShiftState::default(), false);
    assert!(rec.calls.is_empty(), "{:?}", rec.calls);
    assert_eq!(r.redrive, "off (ADR-26 default)");
    assert!(r.redrive_plan.is_empty());

    // A committed `redrive = true` does not turn it on either.
    let committed = toml::from_str::<toml::Value>("[shift]\nredrive = true\n").unwrap();
    let c = build_config(Some(&committed), None, REPO, "L");
    assert!(!c.redrive && c.committed_redrive_ignored);
    let (r, rec) = run(&c, &p, &ShiftState::default(), false);
    assert!(rec.calls.is_empty());
    assert!(
        r.redrive.starts_with("off (ADR-26 default; the committed"),
        "{}",
        r.redrive
    );
}

#[test]
fn watch_redrive_requeues_at_the_queue_head_and_launches_nothing() {
    let p = probes(vec![drain_park("TASK-9", 30), drain_park("TASK-8", 90)]);
    let (r, rec) = run(&cfg_redrive_on(), &p, &ShiftState::default(), false);
    assert_eq!(r.redriven, vec!["TASK-8", "TASK-9"]);
    assert_eq!(rec.requeued.len(), 1);
    let (specs, queue) = &rec.requeued[0];
    assert_eq!(specs, &vec!["TASK-8".to_string(), "TASK-9".to_string()]);
    assert_eq!(queue.user, "joe");
    assert_eq!(queue.role, "implementer");
    // Only the requeue side effect: no wave, no tag, no state write.
    assert_eq!(rec.calls, vec!["requeue"]);
    assert!(rec.spawns.is_empty());

    // Without --execute it is a preview and writes nothing.
    let (r, rec) = run(&cfg_redrive_on(), &p, &ShiftState::default(), true);
    assert!(rec.calls.is_empty(), "{:?}", rec.calls);
    assert!(
        r.redrive.starts_with("on — would re-queue TASK-8, TASK-9"),
        "{}",
        r.redrive
    );
}

// ---------------------------------------------------------------------------
// Every floor holds on the watch path
// ---------------------------------------------------------------------------

#[test]
fn watch_redrive_held_while_a_drain_or_wave_is_live() {
    let parked = vec![drain_park("TASK-9", 60)];
    // A drain holds this clone's lock.
    let mut p = probes(parked.clone());
    p.lock = LockView::Running(77);
    let (r, rec) = run(&cfg_redrive_on(), &p, &ShiftState::default(), false);
    assert!(rec.calls.is_empty(), "{:?}", rec.calls);
    assert!(failing_guard(&r, "redrive-lock-free"));
    assert!(
        r.redrive.starts_with("held: redrive-lock-free"),
        "{}",
        r.redrive
    );

    // Another clone is draining.
    let mut p = probes(parked.clone());
    p.foreign_claim = Some("another clone is draining".to_string());
    let (r, rec) = run(&cfg_redrive_on(), &p, &ShiftState::default(), false);
    assert!(rec.calls.is_empty());
    assert!(failing_guard(&r, "redrive-lock-free"));

    // A shift wave is still alive.
    let mut p = probes(parked);
    p.last_wave_alive = true;
    let state = ShiftState {
        waves: vec![WaveRecord {
            batch: "shift-20260925-0250".to_string(),
            specs: vec!["TASK-1".to_string()],
            at: now() - Duration::minutes(10),
            argv: Vec::new(),
            pid: Some(7),
            pid_start: None,
            log: None,
            outcome: None,
        }],
        ..Default::default()
    };
    let (r, rec) = run(&cfg_redrive_on(), &p, &state, false);
    assert!(rec.calls.is_empty());
    assert!(failing_guard(&r, "redrive-lock-free"));
}

#[test]
fn watch_redrive_held_by_a_tripped_breaker_or_unreadable_state() {
    let p = probes(vec![drain_park("TASK-9", 60)]);
    let state = ShiftState {
        breaker: Some(Breaker {
            tripped_at: now(),
            reason: "2 consecutive shift waves made no progress".to_string(),
            queue_fingerprint: Vec::new(),
        }),
        ..Default::default()
    };
    let (r, rec) = run(&cfg_redrive_on(), &p, &state, false);
    assert!(rec.calls.is_empty(), "{:?}", rec.calls);
    assert!(failing_guard(&r, "redrive-breaker"));

    let mut rec = Rec::default();
    let mut c = ctx(false);
    c.state_error = Some("night-shift state is unreadable".to_string());
    let r = redrive_pass(&cfg_redrive_on(), &p, &ShiftState::default(), &c, &mut rec).unwrap();
    assert!(rec.calls.is_empty());
    assert!(failing_guard(&r, "redrive-state"));
}

#[test]
fn watch_redrive_never_touches_held_attention_non_drain_or_keystone_parks() {
    let held = drain_park("TASK-1", 60);
    let drive = park("TASK-2", "watchdog", Some(ExecutionMode::Drive), 60);
    let no_mode = park("TASK-3", "watchdog", None, 60);
    let mut attention = drain_park("TASK-4", 60);
    attention.attention_reason = Some(aida_core::AttentionReason {
        category: aida_core::PuntCategory::DesignFork,
        detail: "pick one".to_string(),
        lean: None,
        raised_by: None,
        raised_at: now(),
    });
    let mut keystone = drain_park("TASK-5", 60);
    keystone.tags.insert("keystone".to_string());
    let ci_red = park("TASK-6", "ci-red", Some(ExecutionMode::Drain), 60);
    let mut tagged = drain_park("TASK-7", 60);
    tagged.tags.insert("needs-human".to_string());
    let mut p = probes(vec![
        held, drive, no_mode, attention, keystone, ci_red, tagged,
    ]);
    p.held = ["TASK-1".to_string()].into_iter().collect();
    // Even at the cap, a floored park is left exactly as it is.
    let cap: Vec<events::Event> = (1..=3)
        .map(|n| {
            let mut ev = events::Event::new(
                Some("TASK-2".to_string()),
                "",
                EventKind::SpecReDriven {
                    cause: "watchdog".to_string(),
                    attempt: n,
                    max: 3,
                },
            );
            ev.ts = now() - Duration::hours(4 - n as i64);
            ev
        })
        .collect();
    p.redrive_history = Ok(events::RedriveHistory::from_events(&cap));

    let (r, rec) = run(&cfg_redrive_on(), &p, &ShiftState::default(), false);
    assert!(rec.calls.is_empty(), "{:?}", rec.calls);
    assert_eq!(action(&r, "TASK-1"), "leave-merge-held");
    assert_eq!(action(&r, "TASK-2"), "leave-not-drain-mode");
    assert_eq!(action(&r, "TASK-3"), "leave-not-drain-mode");
    assert_eq!(action(&r, "TASK-4"), "leave-for-human");
    assert_eq!(action(&r, "TASK-5"), "leave-keystone");
    assert_eq!(action(&r, "TASK-6"), "leave-for-human");
    assert_eq!(action(&r, "TASK-7"), "leave-for-human");
    assert_eq!(r.redrive, "on — nothing to re-drive");
}

#[test]
fn watch_redrive_evidence_guard_fails_closed() {
    let mut p = probes(vec![drain_park("TASK-9", 60)]);
    p.redrive_history = Err("AIDA_EVENTS_DISABLE is set".to_string());
    let (r, rec) = run(&cfg_redrive_on(), &p, &ShiftState::default(), false);
    assert!(rec.calls.is_empty(), "{:?}", rec.calls);
    assert!(failing_guard(&r, "redrive-evidence"));
    assert_eq!(r.redrive, "held: redrive-evidence");
}

#[test]
fn watch_redrive_unrecorded_attempt_holds_the_rest_of_the_pass() {
    let p = probes(vec![drain_park("TASK-8", 90), drain_park("TASK-9", 30)]);
    let mut rec = Rec {
        unrecorded_from: Some(0),
        ..Default::default()
    };
    let r = redrive_pass(
        &cfg_redrive_on(),
        &p,
        &ShiftState::default(),
        &ctx(false),
        &mut rec,
    )
    .unwrap();
    assert!(rec.requeued.is_empty());
    assert!(r.redriven.is_empty());
    assert!(r.redrive_held.is_some());
    assert_eq!(action(&r, "TASK-8"), crate::supervisor::HELD_UNRECORDED);
    assert_eq!(action(&r, "TASK-9"), crate::supervisor::HELD_UNRECORDED);
}

#[test]
fn watch_redrive_cap_reclassifies_instead_of_a_fourth_attempt() {
    let mut p = probes(vec![drain_park("TASK-9", 30)]);
    let cap: Vec<events::Event> = (1..=3)
        .map(|n| {
            let mut ev = events::Event::new(
                Some("TASK-9".to_string()),
                "",
                EventKind::SpecReDriven {
                    cause: "watchdog".to_string(),
                    attempt: n,
                    max: 3,
                },
            );
            ev.ts = now() - Duration::hours(4 - n as i64);
            ev
        })
        .collect();
    p.redrive_history = Ok(events::RedriveHistory::from_events(&cap));
    let (r, rec) = run(&cfg_redrive_on(), &p, &ShiftState::default(), false);
    assert!(rec.requeued.is_empty());
    assert_eq!(rec.reclassified, vec!["TASK-9"]);
    assert_eq!(r.reclassified, vec!["TASK-9"]);
}

// ---------------------------------------------------------------------------
// No forced claim, no foreground drive
// ---------------------------------------------------------------------------

/// The body of `fn <name>` in `src`, up to the next top-level `fn`.
fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("no fn {name}"));
    let rest = &src[start..];
    let end = rest[1..].find("\nfn ").map(|i| i + 1).unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn watch_redrive_never_uses_force_claim_or_a_foreground_drive() {
    // Behaviour: the re-drive pass has no launch at all. Across a pass that
    // re-queues, one that reclassifies and one that holds, the only side
    // effects are requeue and reclassify; `spawn_wave` (the one launch seam)
    // is never called, so no argv, forced or not, is ever built.
    let mut all = Rec::default();
    let mut cap = probes(vec![drain_park("TASK-7", 30)]);
    cap.redrive_history = Ok(events::RedriveHistory::from_events(
        &(1..=3)
            .map(|n| {
                let mut ev = events::Event::new(
                    Some("TASK-7".to_string()),
                    "",
                    EventKind::SpecReDriven {
                        cause: "watchdog".to_string(),
                        attempt: n,
                        max: 3,
                    },
                );
                ev.ts = now() - Duration::hours(4 - n as i64);
                ev
            })
            .collect::<Vec<_>>(),
    ));
    for p in [probes(vec![drain_park("TASK-9", 60)]), cap] {
        redrive_pass(
            &cfg_redrive_on(),
            &p,
            &ShiftState::default(),
            &ctx(false),
            &mut all,
        )
        .unwrap();
    }
    assert!(all.spawns.is_empty());
    assert!(
        all.calls
            .iter()
            .all(|c| c == "requeue" || c == "reclassify"),
        "{:?}",
        all.calls
    );

    // Wiring: the watch pass reaches re-drive only through the shift's
    // floored pass, never through the manual verb (whose foreground launch
    // is `queue work --force-claim`).
    let src = include_str!("../supervise_cmd.rs");
    let watch = fn_body(src, "run_watch_pass");
    assert!(watch.contains("crate::shift::run_redrive_pass("), "{watch}");
    for banned in [
        "handle_supervise_command",
        "launch_redrive",
        "--force-claim",
        "floors: None",
    ] {
        assert!(!watch.contains(banned), "watch pass uses `{banned}`");
    }
    assert!(!src.contains("\"--force-claim\""));
    let shell = fn_body(include_str!("../shift.rs"), "run_redrive_pass_with");
    assert!(shell.contains("redrive_pass(cfg, &p, &state, &ctx, &mut exec)"));
    assert!(!shell.contains("--force-claim") && !shell.contains("spawn_wave"));
}

// ---------------------------------------------------------------------------
// The production shell on a temp store (no ~/.aida: the config is injected)
// ---------------------------------------------------------------------------

fn temp_store(tmp: &Path) -> aida_core::CachedGitBackend {
    let store = tmp.join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::create_dir_all(tmp.join(".aida")).unwrap();
    aida_core::CachedGitBackend::open(&store, &tmp.join(".aida").join("cache.db")).unwrap()
}

fn add_park(backend: &aida_core::CachedGitBackend, spec: &str, mins_ago: i64) -> uuid::Uuid {
    let r = park_at(
        spec,
        "watchdog",
        Some(ExecutionMode::Drain),
        Utc::now() - Duration::minutes(mins_ago),
    );
    backend.add_requirement(r).unwrap().id
}

fn status(backend: &aida_core::CachedGitBackend, id: uuid::Uuid) -> RequirementStatus {
    backend.get_requirement(&id).unwrap().unwrap().status
}

#[test]
fn watch_redrive_records_the_attempt_before_the_requeue_on_a_real_store() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        (events::EVENTS_DISABLE_ENV, None),
        ("AIDA_USER", Some("joe")),
    ]);
    let tmp = tempfile::tempdir().unwrap();
    let backend = temp_store(tmp.path());
    let id = add_park(&backend, "TASK-9", 60);
    std::fs::write(tmp.path().join(".aida").join("events.jsonl"), "").unwrap();

    // Off: nothing is read or written.
    let r = run_redrive_pass_with(tmp.path(), &backend, &cfg_redrive_off(), false).unwrap();
    assert_eq!(r.redrive, "off (ADR-26 default)");
    assert_eq!(status(&backend, id), RequirementStatus::NeedsAttention);

    // Dry run (watch without --execute): a preview only.
    let r = run_redrive_pass_with(tmp.path(), &backend, &cfg_redrive_on(), true).unwrap();
    assert!(
        r.redrive.starts_with("on — would re-queue TASK-9"),
        "{}",
        r.redrive
    );
    assert_eq!(status(&backend, id), RequirementStatus::NeedsAttention);
    assert!(events::read_all(tmp.path()).is_empty());

    let r = run_redrive_pass_with(tmp.path(), &backend, &cfg_redrive_on(), false).unwrap();
    assert_eq!(r.redriven, vec!["TASK-9"]);
    assert_eq!(status(&backend, id), RequirementStatus::Approved);
    let kinds: Vec<&str> = events::read_all(tmp.path())
        .iter()
        .map(|e| e.kind.name())
        .collect();
    let redriven = kinds
        .iter()
        .position(|k| *k == "SpecReDriven")
        .expect("recorded");
    let requeued = kinds
        .iter()
        .position(|k| *k == "SpecRequeued")
        .expect("trail");
    assert!(redriven < requeued, "{kinds:?}");
    // Back in the queue for the implementer; no drive was launched.
    let queue = backend.queue_list("joe", false).unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].requirement_id, id);
    assert_eq!(queue[0].for_role.as_deref(), Some("implementer"));
    assert!(!tmp.path().join(".aida").join("drain.lock").exists());
}

#[test]
fn watch_redrive_unwritable_events_file_requeues_nothing_on_a_real_store() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        (events::EVENTS_DISABLE_ENV, None),
        ("AIDA_USER", Some("joe")),
    ]);
    let tmp = tempfile::tempdir().unwrap();
    let backend = temp_store(tmp.path());
    let id = add_park(&backend, "TASK-9", 60);
    // The events path is a directory: the evidence cannot be read, and no
    // attempt could be recorded.
    std::fs::create_dir_all(tmp.path().join(".aida").join("events.jsonl")).unwrap();
    let r = run_redrive_pass_with(tmp.path(), &backend, &cfg_redrive_on(), false).unwrap();
    assert!(r.redriven.is_empty());
    assert!(
        r.redrive.starts_with("held: redrive-evidence"),
        "{}",
        r.redrive
    );
    assert_eq!(status(&backend, id), RequirementStatus::NeedsAttention);
    assert!(backend.queue_list("joe", false).unwrap().is_empty());
}

#[test]
fn watch_redrive_real_floors_merge_hold_breaker_drain_lock_and_shift_lock() {
    use fs2::FileExt;
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        (events::EVENTS_DISABLE_ENV, None),
        ("AIDA_USER", Some("joe")),
    ]);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let backend = temp_store(root);
    let held = add_park(&backend, "TASK-1", 60);
    let free = add_park(&backend, "TASK-2", 60);
    std::fs::write(root.join(".aida").join("events.jsonl"), "").unwrap();
    let parked = |backend: &aida_core::CachedGitBackend| {
        [held, free]
            .iter()
            .all(|id| status(backend, *id) == RequirementStatus::NeedsAttention)
    };

    // 1. A live drain lock on this clone (held by this test process).
    let lock = serde_json::json!({
        "pid": std::process::id(),
        "started_at_utc": Utc::now().to_rfc3339(),
        "command": "queue work --auto-complete",
        "host": "test",
    });
    let lock_path = crate::drain_lock::drain_lock_path(root);
    std::fs::write(&lock_path, lock.to_string()).unwrap();
    let r = run_redrive_pass_with(root, &backend, &cfg_redrive_on(), false).unwrap();
    assert!(
        failing_guard(&r, "redrive-lock-free"),
        "{:?}",
        r.redrive_guards
    );
    assert!(parked(&backend));
    std::fs::remove_file(&lock_path).unwrap();

    // 2. A tick holds shift.lock: the pass leaves the parks to it.
    let tick = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(root.join(".aida").join("shift.lock"))
        .unwrap();
    tick.lock_exclusive().unwrap();
    let r = run_redrive_pass_with(root, &backend, &cfg_redrive_on(), false).unwrap();
    assert!(
        r.redrive
            .starts_with("held: a night-shift check is running"),
        "{}",
        r.redrive
    );
    assert!(parked(&backend));
    fs2::FileExt::unlock(&tick).unwrap();
    drop(tick);

    // 3. A tripped breaker in the shift state; then an unreadable state.
    let state = ShiftState {
        breaker: Some(Breaker {
            tripped_at: Utc::now(),
            reason: "2 consecutive shift waves made no progress".to_string(),
            queue_fingerprint: Vec::new(),
        }),
        ..Default::default()
    };
    std::fs::write(state_path(root), serde_json::to_string(&state).unwrap()).unwrap();
    let r = run_redrive_pass_with(root, &backend, &cfg_redrive_on(), false).unwrap();
    assert!(failing_guard(&r, "redrive-breaker"));
    assert!(parked(&backend));
    std::fs::write(state_path(root), "{not json").unwrap();
    let r = run_redrive_pass_with(root, &backend, &cfg_redrive_on(), false).unwrap();
    assert!(failing_guard(&r, "redrive-state"));
    assert!(parked(&backend));
    std::fs::remove_file(state_path(root)).unwrap();

    // 4. A real merge hold on TASK-1: only TASK-2 is re-driven.
    let mut hold = crate::merge_hold::typed_hold(
        41,
        crate::merge_hold::HoldReasonKind::Supervision,
        "operator hold",
        None,
    );
    hold.spec = Some("TASK-1".to_string());
    crate::merge_hold::write_typed_hold(root, &hold).unwrap();
    let r = run_redrive_pass_with(root, &backend, &cfg_redrive_on(), false).unwrap();
    assert_eq!(action(&r, "TASK-1"), "leave-merge-held");
    assert_eq!(r.redriven, vec!["TASK-2"]);
    assert_eq!(status(&backend, held), RequirementStatus::NeedsAttention);
    assert_eq!(status(&backend, free), RequirementStatus::Approved);
}

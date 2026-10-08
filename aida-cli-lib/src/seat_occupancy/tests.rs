//! State-machine tests for seat occupancy (TASK-1607). Processes and the probe
//! are injected; no real peer process is ever a target.
// trace:TASK-1607 | ai:claude

use super::*;
use aida_core::process_probe::{Probe, ProcFacts, ProcessIdentity};
use std::cell::RefCell;
use std::collections::HashMap;

fn pid(n: u32) -> ProcessIdentity {
    ProcessIdentity {
        pid: n,
        start: format!("linux:boot:{}", n * 10),
    }
}

fn proc(n: u32, parent: u32, name: &str) -> ProcFacts {
    ProcFacts {
        identity: pid(n),
        ppid: parent,
        name: name.to_string(),
        cmd: vec![name.to_string()],
    }
}

/// A chain built from (pid, name) pairs, caller first, parents linked.
fn chain(links: &[(u32, &str)]) -> Vec<ProcFacts> {
    links
        .iter()
        .enumerate()
        .map(|(i, (n, name))| proc(*n, links.get(i + 1).map(|l| l.0).unwrap_or(0), name))
        .collect()
}

fn grant(session: &str, seat: &str) -> GrantRef {
    GrantRef {
        subject: session.to_string(),
        id: format!("grant-{session}"),
        session_id: session.to_string(),
        seat: seat.to_string(),
        is_child: false,
    }
}

fn caller(links: &[(u32, &str)], grant: Option<GrantRef>) -> Caller {
    Caller {
        chain: Ok(chain(links)),
        grant,
        lease_scope: None,
    }
}

/// Orchestrator A: `aida` (100) under claude (10) under bash (2) under init (1).
fn orch_a() -> Caller {
    caller(
        &[(100, "aida"), (10, "claude"), (2, "bash"), (1, "init")],
        Some(grant("sess-a", "orchestrator")),
    )
}

/// Orchestrator B on the SAME grant (same shell), different harness (ADR-67).
fn orch_b_same_grant() -> Caller {
    caller(
        &[(200, "aida"), (20, "claude"), (2, "bash"), (1, "init")],
        Some(grant("sess-a", "orchestrator")),
    )
}

/// Orchestrator C: a separate shell and grant.
fn orch_c() -> Caller {
    caller(
        &[(300, "aida"), (30, "claude"), (3, "bash"), (1, "init")],
        Some(grant("sess-c", "orchestrator")),
    )
}

/// A drain child of A: aida (110) under its own claude (11) under A's loop
/// process (100... we use 105 as the loop) under A's claude (10).
fn drain_loop_a() -> Caller {
    caller(
        &[(105, "aida"), (10, "claude"), (2, "bash"), (1, "init")],
        Some(grant("sess-a", "orchestrator")),
    )
}

fn drain_child_of_a(grant: Option<GrantRef>) -> Caller {
    caller(
        &[
            (110, "aida"),
            (12, "bash"),
            (11, "claude"),
            (105, "aida"),
            (10, "claude"),
            (2, "bash"),
            (1, "init"),
        ],
        grant,
    )
}

fn implementer() -> Caller {
    caller(
        &[(400, "aida"), (40, "claude"), (4, "bash"), (1, "init")],
        Some(grant("sess-i", "implementer")),
    )
}

struct World {
    probes: RefCell<HashMap<u32, Probe>>,
    now: RefCell<DateTime<Utc>>,
}

impl World {
    fn new() -> Self {
        World {
            probes: RefCell::new(HashMap::new()),
            now: RefCell::new(
                DateTime::parse_from_rfc3339("2026-10-06T12:00:00Z")
                    .unwrap()
                    .into(),
            ),
        }
    }
    fn set(&self, n: u32, p: Probe) {
        self.probes.borrow_mut().insert(n, p);
    }
    fn advance(&self, secs: i64) {
        let t = *self.now.borrow() + Duration::seconds(secs);
        *self.now.borrow_mut() = t;
    }
    fn with<T>(&self, f: impl FnOnce(&Ctx) -> T) -> T {
        let probes = &self.probes;
        let probe = move |id: &ProcessIdentity| {
            probes
                .borrow()
                .get(&id.pid)
                .cloned()
                .unwrap_or(Probe::Alive)
        };
        let ctx = Ctx {
            now: *self.now.borrow(),
            probe: &probe,
            scope: None,
            requester_valid: Some(&|_, _| true),
        };
        f(&ctx)
    }
}

fn route(spec: &str) -> SeatOp {
    SeatOp::Route {
        spec: Some(spec.to_string()),
        what: format!("enqueue {spec}"),
    }
}

fn loop_start() -> SeatOp {
    SeatOp::LoopStart {
        what: "start a burndown run".into(),
    }
}

fn claimed(w: &World, rec: &mut SeatRecord, c: &Caller) -> ClaimNotice {
    match w.with(|ctx| gate(rec, ctx, c, &route("TASK-1"))) {
        Ok(Gate::Holder { claimed: Some(n) }) => n,
        other => panic!("expected an implicit claim, got {other:?}"),
    }
}

// --- claims -----------------------------------------------------------------

#[test]
fn implicit_claim_announces_seat_generation_anchor_and_release() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    let notice = claimed(&w, &mut rec, &orch_a());
    assert_eq!(notice.generation, 1);
    assert_eq!(notice.anchor_pid, 10, "anchor is the nearest harness");
    let line = notice.to_string();
    for needle in [
        "orchestrator",
        "generation 1",
        "claude pid 10",
        "aida session seat release --seat orchestrator --generation 1",
    ] {
        assert!(line.contains(needle), "C2: `{needle}` missing from: {line}");
    }
    assert_eq!(rec.outbox.last().unwrap().kind, SeatEventKind::Claimed);
}

#[test]
fn holder_reclaims_idempotently() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    let again = w
        .with(|ctx| gate(&mut rec, ctx, &orch_a(), &route("TASK-2")))
        .unwrap();
    assert_eq!(again, Gate::Holder { claimed: None });
    assert_eq!(w.with(|ctx| claim(&mut rec, ctx, &orch_a())).unwrap(), None);
    assert_eq!(rec.generation, 1);
}

#[test]
fn sibling_on_same_grant_is_a_competitor_and_sees_who_holds_the_seat() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    let refusal = w
        .with(|ctx| gate(&mut rec, ctx, &orch_b_same_grant(), &route("TASK-2")))
        .unwrap_err();
    let text = refusal.to_string();
    for needle in [
        "held by another live session",
        "session sess-a",
        "claude pid 10",
        "since 2026-10-06 12:00:00Z",
        "alive",
        "aida session seat release --seat orchestrator --generation 1",
        "aida session seat takeover --seat orchestrator --force",
    ] {
        assert!(
            text.contains(needle),
            "C3: `{needle}` missing from:\n{text}"
        );
    }
    assert_eq!(rec.generation, 1, "a refused claim changes nothing");
}

#[test]
fn concurrent_distinct_orchestrators_only_one_wins() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    assert!(w.with(|ctx| claim(&mut rec, ctx, &orch_c())).is_err());
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &orch_c(), &loop_start()))
        .is_err());
}

#[test]
fn ordinary_work_is_unaffected_but_cannot_start_a_loop() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    assert_eq!(
        w.with(|ctx| gate(&mut rec, ctx, &implementer(), &route("TASK-9")))
            .unwrap(),
        Gate::Pass
    );
    let no_grant = caller(&[(500, "aida"), (5, "bash"), (1, "init")], None);
    assert_eq!(
        w.with(|ctx| gate(&mut rec, ctx, &no_grant, &route("TASK-9")))
            .unwrap(),
        Gate::Pass
    );
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &no_grant, &loop_start()))
        .is_err());
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &no_grant, &SeatOp::HandoffWrite))
        .is_err());
}

#[test]
fn any_direct_grant_may_claim_for_a_loop_start_but_not_for_routing() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    let advisor = caller(
        &[(600, "aida"), (60, "claude"), (6, "bash"), (1, "init")],
        Some(grant("sess-adv", "advisor")),
    );
    assert_eq!(
        w.with(|ctx| gate(&mut rec, ctx, &advisor, &route("TASK-1")))
            .unwrap(),
        Gate::Pass,
        "an advisor's routing is ordinary work"
    );
    assert!(matches!(
        w.with(|ctx| gate(&mut rec, ctx, &advisor, &loop_start()))
            .unwrap(),
        Gate::Holder { claimed: Some(_) }
    ));
}

#[test]
fn child_grant_cannot_claim() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    let mut g = grant("sess-k", "orchestrator");
    g.is_child = true;
    let c = caller(&[(700, "aida"), (70, "claude"), (1, "init")], Some(g));
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &c, &loop_start()))
        .is_err());
    assert_eq!(rec.holder, None);
}

// --- C1: drain-spawned children ----------------------------------------------

fn seat_with_drain(w: &World) -> SeatRecord {
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(w, &mut rec, &orch_a());
    w.with(|ctx| gate(&mut rec, ctx, &drain_loop_a(), &loop_start()))
        .unwrap();
    w.with(|ctx| {
        record_child(&mut rec, ctx, pid(11), Some("TASK-7".into()), "implementer");
    });
    rec
}

#[test]
fn drain_child_completes_its_own_scope_while_parent_holds_the_seat() {
    let w = World::new();
    let mut rec = seat_with_drain(&w);
    // Under --no-human the child carries no grant.
    let child = drain_child_of_a(None);
    for what in ["queue done TASK-7", "status edit TASK-7"] {
        let op = SeatOp::Route {
            spec: Some("TASK-7".into()),
            what: what.into(),
        };
        assert_eq!(
            w.with(|ctx| gate(&mut rec, ctx, &child, &op)).unwrap(),
            Gate::Pass,
            "{what}"
        );
    }
    assert_eq!(rec.generation, 1, "a child's own work never claims");
}

#[test]
fn drain_child_cannot_route_spawn_or_start_a_loop() {
    let w = World::new();
    let mut rec = seat_with_drain(&w);
    let child = drain_child_of_a(None);
    let spawn = SeatOp::Spawn {
        spec: Some("TASK-8".into()),
        what: "spawn a reviewer".into(),
    };
    for op in [route("TASK-8"), spawn, loop_start(), SeatOp::HandoffWrite] {
        let r = w.with(|ctx| gate(&mut rec, ctx, &child, &op)).unwrap_err();
        assert!(r.reason.contains("delegated worker"), "{op:?}: {r}");
    }
    // Even carrying an orchestrator grant, lineage wins: no competing claim.
    let child_with_grant = drain_child_of_a(Some(grant("sess-a", "orchestrator")));
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &child_with_grant, &route("TASK-8")))
        .is_err());
    assert_eq!(rec.generation, 1);
}

#[test]
fn child_falls_back_to_its_lease_scope_when_no_spawn_was_recorded() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    w.with(|ctx| gate(&mut rec, ctx, &drain_loop_a(), &loop_start()))
        .unwrap();
    let mut child = drain_child_of_a(None);
    child.lease_scope = Some("TASK-5".into());
    assert_eq!(
        w.with(|ctx| gate(&mut rec, ctx, &child, &route("TASK-5")))
            .unwrap(),
        Gate::Pass
    );
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &child, &route("TASK-6")))
        .is_err());
}

// --- release, demotion and C4 -------------------------------------------------

#[test]
fn release_empties_the_seat_and_fences_the_former_holder() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    let out = w
        .with(|ctx| release(&mut rec, ctx, &orch_a(), 1, None))
        .unwrap();
    assert_eq!(out, Handover::Released { generation: 2 });
    assert_eq!(rec.holder, None);
    // C4: the former holder cannot implicitly claim a fresh generation...
    let r = w
        .with(|ctx| gate(&mut rec, ctx, &orch_a(), &route("TASK-2")))
        .unwrap_err();
    assert!(r.reason.contains("claim the seat again implicitly"), "{r}");
    assert_eq!(rec.generation, 2);
    // ...even after dropping its grant from the environment.
    let mut env_dropped = orch_a();
    env_dropped.grant = None;
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &env_dropped, &route("TASK-2")))
        .is_err());
    // An explicit claim brings it back.
    let n = w
        .with(|ctx| claim(&mut rec, ctx, &orch_a()))
        .unwrap()
        .unwrap();
    assert_eq!(n.generation, 3);
    assert!(rec.tombstones.is_empty());
}

#[test]
fn stale_generation_and_non_holder_release_refuse() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    assert!(w
        .with(|ctx| release(&mut rec, ctx, &orch_a(), 0, None))
        .is_err());
    assert!(w
        .with(|ctx| release(&mut rec, ctx, &orch_c(), 1, None))
        .is_err());
    // Same grant session from a different harness is not the holder either.
    assert!(w
        .with(|ctx| release(&mut rec, ctx, &orch_b_same_grant(), 1, None))
        .is_err());
    assert_eq!(rec.generation, 1);
}

#[test]
fn holder_lineage_without_the_holders_grant_is_refused() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    let mut scrubbed = orch_a();
    scrubbed.grant = None;
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &scrubbed, &route("TASK-2")))
        .is_err());
}

// --- liveness ------------------------------------------------------------------

#[test]
fn dead_holder_is_replaced_silently() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    w.set(10, Probe::Dead("gone".into()));
    let n = claimed(&w, &mut rec, &orch_c());
    assert_eq!(n.how, ClaimKind::DeadReplacement);
    assert_eq!(n.generation, 2);
    assert_eq!(
        rec.outbox.last().unwrap().kind,
        SeatEventKind::DeadHolderReplaced
    );
    assert!(rec.tombstones.is_empty(), "a dead holder needs no fence");
}

#[test]
fn unknown_holder_blocks_claims_and_takeover() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    w.set(10, Probe::Unknown("permission denied".into()));
    let r = w
        .with(|ctx| gate(&mut rec, ctx, &orch_c(), &route("TASK-2")))
        .unwrap_err();
    assert!(r.reason.contains("cannot be verified"), "{r}");
    assert!(w
        .with(|ctx| takeover(&mut rec, ctx, &orch_c(), Duration::seconds(60), None))
        .is_err());
    assert_eq!(rec.generation, 1);
}

#[test]
fn detached_loop_keeps_the_seat_alive_after_its_harness_exits() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    w.with(|ctx| gate(&mut rec, ctx, &drain_loop_a(), &loop_start()))
        .unwrap();
    w.set(10, Probe::Dead("harness exited".into()));
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &orch_c(), &route("TASK-2")))
        .is_err());
    w.set(105, Probe::Dead("loop exited".into()));
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &orch_c(), &route("TASK-2")))
        .is_ok());
}

#[test]
fn unidentifiable_caller_refuses() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    let c = Caller {
        chain: Err("cannot read /proc/1/stat".into()),
        grant: Some(grant("sess-a", "orchestrator")),
        lease_scope: None,
    };
    let r = w
        .with(|ctx| gate(&mut rec, ctx, &c, &route("TASK-1")))
        .unwrap_err();
    assert!(r.reason.contains("cannot identify"), "{r}");
    assert_eq!(rec.holder, None);
}

// --- graceful takeover -------------------------------------------------------------

fn pending(w: &World, rec: &mut SeatRecord, c: &Caller) -> TakeoverRequest {
    match w.with(|ctx| takeover(rec, ctx, c, Duration::seconds(120), None)) {
        Ok(TakeoverStep::Pending { request, .. }) => request,
        other => panic!("expected a pending request, got {other:?}"),
    }
}

#[test]
fn graceful_ack_transfers_and_demotes_the_old_holder() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    w.with(|ctx| record_child(&mut rec, ctx, pid(11), Some("TASK-7".into()), "implementer"));
    let req = pending(&w, &mut rec, &orch_c());
    // New dispatch stops while the request is pending; the handoff can still be written.
    let r = w
        .with(|ctx| gate(&mut rec, ctx, &orch_a(), &route("TASK-2")))
        .unwrap_err();
    assert!(
        r.to_string().contains(&format!("--request {}", req.id)),
        "{r}"
    );
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &orch_a(), &SeatOp::HandoffWrite))
        .is_ok());

    let out = w
        .with(|ctx| ack(&mut rec, ctx, &orch_a(), &req.id, 1))
        .unwrap();
    assert_eq!(
        out,
        Handover::Transferred {
            to_session: "sess-c".into(),
            generation: 2
        }
    );
    assert_eq!(
        w.with(|ctx| poll_takeover(&mut rec, ctx, &req.id, "sess-c")),
        WaitState::Granted { generation: 2 }
    );
    // The demoted holder is fenced even without its grant; its child keeps working.
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &orch_a(), &route("TASK-2")))
        .is_err());
    assert_eq!(
        w.with(|ctx| gate(&mut rec, ctx, &drain_child_of_a(None), &route("TASK-7")))
            .unwrap(),
        Gate::Pass
    );
    let transferred = rec
        .outbox
        .iter()
        .find(|e| e.kind == SeatEventKind::Transferred)
        .unwrap();
    assert_eq!(transferred.from_session.as_deref(), Some("sess-a"));
    assert_eq!(transferred.to_session.as_deref(), Some("sess-c"));
    assert_eq!(
        (transferred.generation_from, transferred.generation_to),
        (1, 2)
    );
    assert!(transferred.in_flight.iter().any(|s| s.contains("TASK-7")));
}

#[test]
fn spoofed_or_stale_acks_have_no_effect() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    let req = pending(&w, &mut rec, &orch_c());
    // Wrong request id, stale generation, the requester itself, a sibling on
    // the holder's grant: none transfer.
    assert!(w
        .with(|ctx| ack(&mut rec, ctx, &orch_a(), "not-the-request", 1))
        .is_err());
    assert!(w
        .with(|ctx| ack(&mut rec, ctx, &orch_a(), &req.id, 0))
        .is_err());
    assert!(w
        .with(|ctx| ack(&mut rec, ctx, &orch_c(), &req.id, 1))
        .is_err());
    assert!(w
        .with(|ctx| ack(&mut rec, ctx, &orch_b_same_grant(), &req.id, 1))
        .is_err());
    assert_eq!(rec.holder.as_ref().unwrap().grant_session, "sess-a");
    assert_eq!(rec.generation, 1);
}

#[test]
fn silent_holder_times_out_without_a_kill_and_keeps_its_generation() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    let req = pending(&w, &mut rec, &orch_c());
    // A mail notification is not an ack: nothing changes until the deadline.
    w.advance(119);
    assert_eq!(
        w.with(|ctx| poll_takeover(&mut rec, ctx, &req.id, "sess-c")),
        WaitState::Waiting
    );
    w.advance(1);
    assert_eq!(
        w.with(|ctx| poll_takeover(&mut rec, ctx, &req.id, "sess-c")),
        WaitState::TimedOut
    );
    assert_eq!(rec.request, None);
    assert_eq!(rec.holder.as_ref().unwrap().grant_session, "sess-a");
    assert_eq!(rec.generation, 1);
    assert!(rec
        .outbox
        .iter()
        .any(|e| e.kind == SeatEventKind::TakeoverTimedOut));
    // The holder can dispatch again.
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &orch_a(), &route("TASK-2")))
        .is_ok());
}

#[test]
fn interrupted_request_resumes_with_its_original_deadline() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    let req = pending(&w, &mut rec, &orch_c());
    w.advance(60);
    let resumed = w
        .with(|ctx| {
            takeover(
                &mut rec,
                ctx,
                &orch_c(),
                Duration::seconds(120),
                Some(&req.id),
            )
        })
        .unwrap();
    assert_eq!(
        resumed,
        TakeoverStep::Pending {
            request: req.clone(),
            created: false
        },
        "same request, same deadline: grace is never reset"
    );
    // A different requester gets the existing request's identity, not a new one.
    let other = caller(
        &[(800, "aida"), (80, "claude"), (8, "bash"), (1, "init")],
        Some(grant("sess-d", "orchestrator")),
    );
    let r = w
        .with(|ctx| takeover(&mut rec, ctx, &other, Duration::seconds(120), None))
        .unwrap_err();
    assert!(r.reason.contains(&req.id), "{r}");
    // And it cannot resume someone else's request.
    assert!(w
        .with(|ctx| takeover(&mut rec, ctx, &other, Duration::seconds(120), Some(&req.id)))
        .is_err());
}

#[test]
fn release_with_a_pending_request_transfers_or_cancels() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    let req = pending(&w, &mut rec, &orch_c());
    assert!(
        w.with(|ctx| release(&mut rec, ctx, &orch_a(), 1, None))
            .is_err(),
        "a pending request needs its id"
    );
    // Requester died: release cancels the request and empties the seat.
    w.set(30, Probe::Dead("exited".into()));
    let out = w
        .with(|ctx| release(&mut rec, ctx, &orch_a(), 1, Some(&req.id)))
        .unwrap();
    assert_eq!(out, Handover::Released { generation: 2 });
    assert!(rec
        .outbox
        .iter()
        .any(|e| e.kind == SeatEventKind::TakeoverCancelled));
}

#[test]
fn ack_toward_a_dead_requester_keeps_the_holder() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    let req = pending(&w, &mut rec, &orch_c());
    w.set(30, Probe::Dead("exited".into()));
    let r = w
        .with(|ctx| ack(&mut rec, ctx, &orch_a(), &req.id, 1))
        .unwrap_err();
    assert!(r.reason.contains("still hold the seat"), "{r}");
    assert_eq!(rec.holder.as_ref().unwrap().grant_session, "sess-a");
    assert_eq!(rec.request, None);
}

#[test]
fn takeover_of_an_empty_or_dead_seat_claims_immediately() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    assert!(matches!(
        w.with(|ctx| takeover(&mut rec, ctx, &orch_c(), Duration::seconds(5), None))
            .unwrap(),
        TakeoverStep::Claimed(_)
    ));
    w.set(30, Probe::Dead("exited".into()));
    assert!(matches!(
        w.with(|ctx| takeover(&mut rec, ctx, &orch_a(), Duration::seconds(5), None))
            .unwrap(),
        TakeoverStep::Claimed(ClaimNotice {
            how: ClaimKind::DeadReplacement,
            ..
        })
    ));
}

#[test]
fn every_event_has_a_unique_op_id() {
    let w = World::new();
    let mut rec = SeatRecord::empty("orchestrator");
    claimed(&w, &mut rec, &orch_a());
    let req = pending(&w, &mut rec, &orch_c());
    w.with(|ctx| ack(&mut rec, ctx, &orch_a(), &req.id, 1))
        .unwrap();
    let ids: std::collections::HashSet<_> = rec.outbox.iter().map(|e| e.op_id.clone()).collect();
    assert_eq!(ids.len(), rec.outbox.len());
    let kinds: Vec<_> = rec.outbox.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        vec![
            SeatEventKind::Claimed,
            SeatEventKind::TakeoverRequested,
            SeatEventKind::TakeoverAcknowledged,
            SeatEventKind::Transferred
        ]
    );
}

// trace:TASK-1607 | ai:codex
#[test]
fn shell_one_shot_route_and_spawn_do_not_fence_a_later_harness() {
    let w = World::new();
    let mut rec = SeatRecord::empty(ORCHESTRATOR);
    let shell = caller(
        &[(900, "aida"), (2, "bash"), (1, "init")],
        Some(grant("sess-a", ORCHESTRATOR)),
    );
    for op in [
        route("TASK-7"),
        SeatOp::Spawn {
            spec: Some("TASK-7".into()),
            what: "spawn implementer".into(),
        },
    ] {
        assert_eq!(
            w.with(|ctx| gate(&mut rec, ctx, &shell, &op)).unwrap(),
            Gate::Pass
        );
        assert_eq!(
            rec,
            SeatRecord::empty(ORCHESTRATOR),
            "one-shot shell must not install a holder or event"
        );
    }
    assert!(matches!(
        w.with(|ctx| gate(&mut rec, ctx, &orch_a(), &route("TASK-7")))
            .unwrap(),
        Gate::Holder { claimed: Some(_) }
    ));
    // A sibling harness, including one on the same grant, is still refused.
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &orch_b_same_grant(), &route("TASK-8")))
        .is_err());
}

#[test]
fn shell_route_passes_with_live_holder_but_loop_and_handoff_still_refuse() {
    let w = World::new();
    let mut rec = SeatRecord::empty(ORCHESTRATOR);
    claimed(&w, &mut rec, &orch_a());
    let shell = caller(
        &[(900, "aida"), (2, "bash"), (1, "init")],
        Some(grant("shell-session", ORCHESTRATOR)),
    );
    assert_eq!(
        w.with(|ctx| gate(&mut rec, ctx, &shell, &route("TASK-7")))
            .unwrap(),
        Gate::Pass
    );
    for op in [loop_start(), SeatOp::HandoffWrite] {
        assert!(w.with(|ctx| gate(&mut rec, ctx, &shell, &op)).is_err());
    }
    assert_eq!(rec.generation, 1);
}

#[test]
fn loop_anchored_route_still_claims_implicitly() {
    let w = World::new();
    let mut rec = SeatRecord::empty(ORCHESTRATOR);
    let mut c = caller(
        &[(901, "aida"), (900, "aida"), (2, "bash")],
        Some(grant("loop-session", ORCHESTRATOR)),
    );
    c.chain.as_mut().unwrap()[1].cmd = vec![
        "aida".into(),
        "queue".into(),
        "work".into(),
        "--auto-complete".into(),
    ];
    assert!(matches!(
        w.with(|ctx| gate(&mut rec, ctx, &c, &route("TASK-7")))
            .unwrap(),
        Gate::Holder { claimed: Some(_) }
    ));
}

#[test]
fn persisted_drain_child_stays_fenced_after_reparenting_and_holder_exit() {
    let w = World::new();
    let mut rec = seat_with_drain(&w);
    let mut child = drain_child_of_a(None);
    child.chain.as_mut().unwrap().truncate(3); // self, wrapper, own harness
    w.set(10, Probe::Dead("parent exited".into()));
    w.set(105, Probe::Dead("loop exited".into()));
    assert_eq!(
        w.with(|ctx| gate(&mut rec, ctx, &child, &route("TASK-7")))
            .unwrap(),
        Gate::Pass
    );
    assert!(w
        .with(|ctx| gate(&mut rec, ctx, &child, &route("TASK-8")))
        .is_err());
    // A reused PID does not inherit the persisted child's authority.
    child.chain.as_mut().unwrap()[2].identity.start = "different-start".into();
    let rc = ResolvedCaller::resolve(&child).unwrap();
    assert_eq!(classify(&rec, &child, &rc), Relation::Ordinary);
}

// trace:TASK-1607 | ai:codex
#[test]
fn shell_one_shot_exemption_preserves_unknown_and_demotion_refusals() {
    let w = World::new();
    let mut rec = SeatRecord::empty(ORCHESTRATOR);
    let shell = caller(
        &[(900, "aida"), (2, "bash"), (1, "init")],
        Some(grant("shell-session", ORCHESTRATOR)),
    );
    w.set(2, Probe::Unknown("permission denied".into()));
    let err = w
        .with(|ctx| gate(&mut rec, ctx, &shell, &route("TASK-7")))
        .unwrap_err();
    assert!(
        err.reason.contains("cannot verify one-shot caller anchor"),
        "{err}"
    );
    assert_eq!(rec, SeatRecord::empty(ORCHESTRATOR));
    w.set(2, Probe::Alive);
    w.with(|ctx| claim(&mut rec, ctx, &shell)).unwrap();
    w.with(|ctx| release(&mut rec, ctx, &shell, 1, None))
        .unwrap();
    let err = w
        .with(|ctx| gate(&mut rec, ctx, &shell, &route("TASK-7")))
        .unwrap_err();
    assert!(err.reason.contains("demoted"), "{err}");
}

// trace:TASK-1607 | ai:codex
#[test]
fn unpolled_deadlines_fence_transfer_not_the_holder() {
    for action in ["ack", "release", "gate", "resume"] {
        for elapsed in [119, 120, 121] {
            let w = World::new();
            let mut rec = SeatRecord::empty("orchestrator");
            claimed(&w, &mut rec, &orch_a());
            let req = pending(&w, &mut rec, &orch_c());
            w.advance(elapsed);
            // No poller reconciles this request: each entry must do so itself.
            let ok = w.with(|ctx| match action {
                "ack" => ack(&mut rec, ctx, &orch_a(), &req.id, 1).is_ok(),
                "release" => release(&mut rec, ctx, &orch_a(), 1, Some(&req.id)).is_ok(),
                "gate" => gate(&mut rec, ctx, &orch_a(), &route("TASK-2")).is_ok(),
                _ => takeover(
                    &mut rec,
                    ctx,
                    &orch_c(),
                    Duration::seconds(900),
                    Some(&req.id),
                )
                .is_ok(),
            });
            assert_eq!(
                ok,
                if action == "gate" {
                    elapsed >= 120
                } else {
                    elapsed < 120
                },
                "{action} at {elapsed}"
            );
            if elapsed >= 120 {
                assert!(rec.request.is_none());
                assert_eq!(rec.generation, 1);
                assert_eq!(rec.holder.as_ref().unwrap().grant_session, "sess-a");
                assert!(rec.tombstones.is_empty());
                assert!(w
                    .with(|ctx| gate(&mut rec, ctx, &orch_a(), &route("TASK-2")))
                    .is_ok());
                assert_eq!(
                    rec.outbox
                        .iter()
                        .filter(|e| e.kind == SeatEventKind::TakeoverTimedOut)
                        .count(),
                    1
                );
                let fresh = pending(&w, &mut rec, &orch_c());
                assert_ne!(fresh.id, req.id);
                assert!(w
                    .with(|ctx| ack(&mut rec, ctx, &orch_a(), &req.id, 1))
                    .is_err());
                assert_eq!(rec.request.as_ref().unwrap().id, fresh.id);
            }
        }
    }
}

// trace:TASK-1607 | ai:codex
#[test]
fn invalid_or_unknown_authority_cancels_ack_and_release_without_demotion() {
    for release_request in [false, true] {
        for unknown in [false, true] {
            let w = World::new();
            let mut rec = SeatRecord::empty("orchestrator");
            claimed(&w, &mut rec, &orch_a());
            let req = pending(&w, &mut rec, &orch_c());
            w.with(|ctx| {
                let invalid = |r: &TakeoverRequest, seat: &str| {
                    assert_eq!(r, &req);
                    assert_eq!(seat, "orchestrator");
                    false
                };
                let ctx = Ctx {
                    requester_valid: if unknown { None } else { Some(&invalid) },
                    ..*ctx
                };
                let result = if release_request {
                    release(&mut rec, &ctx, &orch_a(), 1, Some(&req.id))
                } else {
                    ack(&mut rec, &ctx, &orch_a(), &req.id, 1)
                };
                assert!(result.is_err());
            });
            assert_eq!(rec.generation, 1);
            assert_eq!(rec.holder.as_ref().unwrap().grant_session, "sess-a");
            assert!(rec.tombstones.is_empty());
            assert!(rec.request.is_none());
            assert!(!rec
                .outbox
                .iter()
                .any(|e| e.kind == SeatEventKind::Transferred));
            assert!(w
                .with(|ctx| gate(&mut rec, ctx, &orch_a(), &route("TASK-2")))
                .is_ok());
        }
    }
}

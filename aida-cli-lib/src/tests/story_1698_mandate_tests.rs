//! STORY-1698: the fire-fighting mandate — one test per acceptance bullet and
//! per advisor ruling on the sketch.
// trace:STORY-1698 | ai:claude

use super::*;
use crate::pr_ship::{AuthorSession, ShipApprovalPolicy};
use crate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};

const HEAD: &str = "1aca4e3e9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OLD: &str = "08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ADVISOR: &str = "session:advisor-shipper";
const REVIEWER: &str = "session:independent-reviewer";

fn now() -> DateTime<Utc> {
    "2026-10-15T12:00:00Z".parse().unwrap()
}

fn human_ok() -> Result<()> {
    Ok(())
}

fn no_tty() -> Result<()> {
    bail!("role grants require a human at an interactive TTY")
}

fn active(include_supervised: bool) -> Mandate {
    Mandate {
        id: "6f1c1d6e-0000-4000-8000-000000000001".into(),
        principal: "joe".into(),
        seat: MANDATE_SEAT.into(),
        scope: SCOPE_SHIP_HELD.into(),
        include_supervised,
        reason: Some("fire".into()),
        issued_at: now() - Duration::hours(1),
        expires_at: now() + Duration::hours(7),
        revoked_at: None,
        revoked_by: None,
    }
}

fn approval_by(sha: &str, identity: &str) -> RecordedVerdict {
    RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(sha.into()),
        recorded_at: Some("2026-10-15T10:00:00Z".into()),
        attestation: Some(RecorderAttestation {
            sha: sha.into(),
            human_at_tty: true,
            receipt_id: Some("rcpt".into()),
            identity: vec![identity.into()],
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn approval(sha: &str) -> RecordedVerdict {
    approval_by(sha, REVIEWER)
}

/// A policy under which every attestation in `verdicts` re-validated; the
/// advisor runs the merge.
fn policy(verdicts: &[RecordedVerdict]) -> ShipApprovalPolicy {
    let mut p = ShipApprovalPolicy {
        reviewed_committed_at: Some("2026-10-15T09:00:00Z".parse().unwrap()),
        shipper: vec![ADVISOR.into()],
        ..Default::default()
    };
    for v in verdicts {
        if let Some(a) = v.attestation.as_ref() {
            if let Some(k) = a.authority_key(v.recorded_at.as_deref()) {
                p.verified_authority.insert(k, a.identity.clone());
            }
        }
    }
    p
}

fn spec(id: &str) -> ShipSpec {
    let mut req = aida_core::Requirement::new("Improve a list".into(), "fixture".into());
    req.req_type = aida_core::RequirementType::Task;
    req.execution_mode = Some(aida_core::ExecutionMode::Drive);
    (id.into(), Some(req))
}

fn tagged(id: &str, tag: &str) -> ShipSpec {
    let (id, mut req) = spec(id);
    req.as_mut().unwrap().tags.insert(tag.into());
    (id, req)
}

struct Case {
    state: MandateState,
    grant: Option<(&'static str, &'static str)>,
    specs: Vec<ShipSpec>,
    specs_resolved: bool,
    candidates: Vec<RecordedVerdict>,
    policy: ShipApprovalPolicy,
    head: Option<&'static str>,
    override_stale: bool,
    now: DateTime<Utc>,
}

impl Case {
    /// The happy path: an active mandate, the minting human's advisor seat,
    /// an ordinary spec, an independent attested approval at the head.
    fn ok() -> Self {
        let candidates = vec![approval(HEAD)];
        Self {
            state: MandateState::Active(active(false)),
            grant: Some(("advisor", "joe")),
            specs: vec![spec("TASK-1")],
            specs_resolved: true,
            policy: policy(&candidates),
            candidates,
            head: Some(HEAD),
            override_stale: false,
            now: now(),
        }
    }

    fn decide(&self) -> std::result::Result<String, MandateRefusal> {
        ship_decision(&ShipCheck {
            state: &self.state,
            now: self.now,
            grant: self.grant,
            specs: &self.specs,
            specs_resolved: self.specs_resolved,
            candidates: &self.candidates,
            policy: &self.policy,
            head: self.head,
            override_stale_approval: self.override_stale,
        })
    }
}

// ── Acceptance: an agent cannot start a mandate; a human can ───────────────

#[test]
fn an_agent_without_a_tty_cannot_start_or_stop_a_mandate() {
    let tmp = tempfile::tempdir().unwrap();
    let err = start(tmp.path(), &no_tty, "joe", DEFAULT_TTL, None, false, now()).unwrap_err();
    assert!(
        format!("{err:#}").contains("human at an interactive terminal"),
        "{err:#}"
    );
    assert!(
        load_all(tmp.path()).is_empty(),
        "a refused start writes nothing"
    );

    start(
        tmp.path(),
        &human_ok,
        "joe",
        DEFAULT_TTL,
        None,
        false,
        now(),
    )
    .unwrap();
    assert!(stop(tmp.path(), &no_tty, "agent", now()).is_err());
    assert!(matches!(
        state_at(&load_all(tmp.path()), now()),
        MandateState::Active(_)
    ));
}

/// The production floor is `require_direct_human`, which refuses an
/// AIDA-managed agent session even before it looks at the terminal.
#[test]
fn require_direct_human_refuses_a_managed_agent_session() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::set(&[("AIDA_AGENT_NAME", "drain-worker")]);
    let gate = || crate::seat_authority::require_direct_human();
    assert!(start(tmp.path(), &gate, "joe", DEFAULT_TTL, None, false, now()).is_err());
    assert!(load_all(tmp.path()).is_empty());
}

#[test]
fn a_human_can_start_a_mandate_and_it_is_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    let m = start(
        tmp.path(),
        &human_ok,
        "joe",
        Duration::hours(8),
        Some("outage".into()),
        false,
        now(),
    )
    .unwrap();
    assert_eq!(m.principal, "joe");
    assert_eq!(m.seat, "advisor");
    assert_eq!(m.scope, "ship-held");
    assert_eq!(m.expires_at, now() + Duration::hours(8));
    assert_eq!(m.reason.as_deref(), Some("outage"));
    assert!(tmp.path().join(format!("{}.json", m.id)).is_file());
    assert_eq!(
        state_at(&load_all(tmp.path()), now()),
        MandateState::Active(m)
    );
}

// ── Ruling: 24h cap, no extension ──────────────────────────────────────────

#[test]
fn ttl_is_capped_at_24h_and_a_mandate_cannot_be_extended() {
    assert_eq!(parse_ttl("24h").unwrap(), Duration::hours(24));
    assert_eq!(parse_ttl("90m").unwrap(), Duration::minutes(90));
    assert!(parse_ttl("25h").is_err());
    assert!(parse_ttl("0h").is_err());
    assert!(parse_ttl("8").is_err());
    let tmp = tempfile::tempdir().unwrap();
    assert!(start(
        tmp.path(),
        &human_ok,
        "joe",
        Duration::hours(25),
        None,
        false,
        now()
    )
    .is_err());
    start(
        tmp.path(),
        &human_ok,
        "joe",
        Duration::hours(2),
        None,
        false,
        now(),
    )
    .unwrap();
    let err = start(
        tmp.path(),
        &human_ok,
        "joe",
        Duration::hours(2),
        None,
        false,
        now(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("cannot be extended"), "{err}");
}

// ── Acceptance: the advisor ships under an active mandate ──────────────────

#[test]
fn advisor_ships_a_held_pr_with_attested_independent_approval() {
    assert_eq!(Case::ok().decide(), Ok(active(false).id));
}

/// The cover is what lets the hold gate release for a non-human caller, and
/// a supervision hold then releases (the approval gate runs separately).
#[test]
fn an_active_cover_satisfies_the_hold_release_gates() {
    let hold = crate::merge_hold::typed_hold(
        7,
        crate::merge_hold::HoldReasonKind::Supervision,
        "operator review",
        None,
    );
    assert!(crate::pr_ship::ship_hold_gate(Some(&hold), false, 7, |_| unreachable!()).is_some());
    assert!(crate::pr_ship::ship_hold_gate(Some(&hold), true, 7, |_| unreachable!()).is_none());
    assert!(crate::pr_ship::ship_hold_release_refusal_for(true, false, true, 7).is_none());
}

// ── Acceptance: refused cases ──────────────────────────────────────────────

#[test]
fn an_expired_mandate_is_refused_at_ship_time() {
    let mut c = Case::ok();
    // State computed earlier, clock moved past expiry: still refused.
    c.now = now() + Duration::hours(8);
    assert_eq!(
        c.decide(),
        Err(MandateRefusal::Expired {
            id: active(false).id
        })
    );
    // And read from records at that instant, it is Expired outright.
    let mut c = Case::ok();
    c.state = state_at(&[active(false)], now() + Duration::hours(8));
    assert!(matches!(c.state, MandateState::Expired(_)));
    assert!(matches!(c.decide(), Err(MandateRefusal::Expired { .. })));
}

#[test]
fn a_missing_attestation_is_refused() {
    let mut c = Case::ok();
    let mut v = approval(HEAD);
    v.attestation = None;
    c.candidates = vec![v];
    assert!(matches!(c.decide(), Err(MandateRefusal::Approval(_))));
    c.candidates = Vec::new();
    assert!(matches!(c.decide(), Err(MandateRefusal::Approval(_))));
}

#[test]
fn an_approval_at_a_stale_sha_is_refused() {
    let mut c = Case::ok();
    c.candidates = vec![approval(OLD)];
    c.policy = policy(&c.candidates);
    assert!(matches!(c.decide(), Err(MandateRefusal::StaleApproval(_))));
}

#[test]
fn an_advisor_authored_pr_is_refused() {
    let mut c = Case::ok();
    c.policy.author_sessions = vec![AuthorSession {
        label: "advisor lease".into(),
        tokens: vec![ADVISOR.into()],
    }];
    assert!(matches!(
        c.decide(),
        Err(MandateRefusal::AdvisorAuthored { .. })
    ));
}

#[test]
fn an_approval_by_the_implementer_is_refused() {
    let mut c = Case::ok();
    c.policy.author_sessions = vec![AuthorSession {
        label: "implementer lease".into(),
        tokens: vec![REVIEWER.into()],
    }];
    assert!(matches!(c.decide(), Err(MandateRefusal::Approval(_))));
}

#[test]
fn excluded_spec_classes_are_refused_without_include_supervised() {
    for tag in [
        "security",
        "containment",
        "review-authority",
        "merge-authority",
    ] {
        let mut c = Case::ok();
        c.specs = vec![spec("TASK-1"), tagged("TASK-2", tag)];
        assert!(
            matches!(c.decide(), Err(MandateRefusal::Excluded { ref spec, .. }) if spec == "TASK-2"),
            "{tag}"
        );
        c.state = MandateState::Active(active(true));
        assert!(c.decide().is_ok(), "--include-supervised lifts {tag}");
    }
}

#[test]
fn a_non_advisor_seat_is_never_covered() {
    for grant in [
        Some(("reviewer", "joe")),
        Some(("implementer", "joe")),
        None,
    ] {
        let mut c = Case::ok();
        c.grant = grant;
        assert!(
            matches!(c.decide(), Err(MandateRefusal::NotAdvisor { .. })),
            "{grant:?}"
        );
    }
    let mut c = Case::ok();
    c.grant = Some(("advisor", "mallory"));
    assert!(matches!(
        c.decide(),
        Err(MandateRefusal::PrincipalMismatch { .. })
    ));
}

#[test]
fn unresolved_specs_fail_closed() {
    let mut c = Case::ok();
    c.specs_resolved = false;
    assert_eq!(c.decide(), Err(MandateRefusal::SpecsUnresolved));
    let mut c = Case::ok();
    c.specs = Vec::new();
    assert_eq!(c.decide(), Err(MandateRefusal::SpecsUnresolved));
    let mut c = Case::ok();
    c.specs = vec![("TASK-404".into(), None)];
    assert_eq!(c.decide(), Err(MandateRefusal::SpecsUnresolved));
}

// ── Acceptance: stop takes effect immediately; ledger entries exist ────────

#[test]
fn stop_takes_effect_immediately() {
    let tmp = tempfile::tempdir().unwrap();
    let m = start(
        tmp.path(),
        &human_ok,
        "joe",
        DEFAULT_TTL,
        None,
        false,
        now(),
    )
    .unwrap();
    let mut c = Case::ok();
    c.state = state_at(&load_all(tmp.path()), now());
    assert_eq!(c.decide(), Ok(m.id.clone()));
    let stopped = stop(tmp.path(), &human_ok, "joe", now()).unwrap();
    assert_eq!(stopped.len(), 1);
    c.state = state_at(&load_all(tmp.path()), now());
    assert_eq!(c.decide(), Err(MandateRefusal::Stopped { id: m.id }));
}

#[test]
fn a_mandate_release_is_recorded_in_the_hold_ledger_and_events() {
    let tmp = tempfile::tempdir().unwrap();
    let hold = crate::merge_hold::typed_hold(
        42,
        crate::merge_hold::HoldReasonKind::Supervision,
        "operator review",
        None,
    );
    crate::merge_hold::record_mandate_clearance(tmp.path(), &hold, "joe", "M-1", None).unwrap();
    let clearance: serde_json::Value = serde_json::from_slice(
        &std::fs::read(crate::merge_hold::clearance_path(tmp.path(), 42)).unwrap(),
    )
    .unwrap();
    assert_eq!(clearance["cleared_by"], "advisor:joe");
    assert_eq!(clearance["released_under_mandate"], "M-1");
    assert_eq!(release_note("M-1"), "released under mandate M-1 by advisor");

    let m = active(false);
    emit_changed(tmp.path(), &m, true);
    let mut stopped = m.clone();
    stopped.revoked_at = Some(now());
    emit_changed(tmp.path(), &stopped, false);
    let events: Vec<_> = crate::events::read_all(tmp.path())
        .into_iter()
        .filter_map(|e| match e.kind {
            crate::events::EventKind::MandateChanged { id, active, .. } => Some((id, active)),
            _ => None,
        })
        .collect();
    assert_eq!(events, vec![(m.id.clone(), true), (m.id, false)]);
}

// ── Ruling (b): operator-only is ALWAYS human-only ─────────────────────────

#[test]
fn operator_only_specs_are_never_covered_even_with_include_supervised() {
    let mut c = Case::ok();
    c.state = MandateState::Active(active(true));
    c.specs = vec![tagged("TASK-3", OPERATOR_ONLY_TAG)];
    assert_eq!(
        c.decide(),
        Err(MandateRefusal::OperatorOnly {
            spec: "TASK-3".into()
        })
    );
    let (id, mut req) = spec("TASK-4");
    req.as_mut().unwrap().execution_mode = Some(aida_core::ExecutionMode::Operator);
    c.specs = vec![(id, req)];
    assert_eq!(
        c.decide(),
        Err(MandateRefusal::OperatorOnly {
            spec: "TASK-4".into()
        })
    );
}

// ── Ruling (c) ─────────────────────────────────────────────────────────────

#[test]
fn an_approval_recorded_by_the_shipping_session_is_refused() {
    let mut c = Case::ok();
    c.candidates = vec![approval_by(HEAD, ADVISOR)];
    c.policy = policy(&c.candidates);
    assert_eq!(c.decide(), Err(MandateRefusal::ApproverIsShipper));
}

#[test]
fn override_stale_approval_is_refused_under_a_mandate() {
    let mut c = Case::ok();
    c.override_stale = true;
    assert_eq!(c.decide(), Err(MandateRefusal::OverrideStale));
}

#[test]
fn a_label_only_hold_stays_human_only_under_a_mandate() {
    // Authority granted (as a mandate cover grants it), but a label-only
    // hold leaves no record to clear against: refused.
    assert!(crate::pr_ship::ship_hold_release_refusal_for(false, true, true, 9).is_some());
}

#[test]
fn a_recusal_hold_is_refused_under_a_mandate() {
    let mut hold = crate::merge_hold::typed_hold(
        9,
        crate::merge_hold::HoldReasonKind::Recusal,
        "advisor recused",
        Some(HEAD.into()),
    );
    hold.recused_principals = vec![crate::merge_hold::PrincipalIdentity::human("joe")];
    assert!(crate::pr_ship::ship_hold_gate(Some(&hold), true, 9, |_| unreachable!()).is_some());
}

// ── Statusline ─────────────────────────────────────────────────────────────

#[test]
fn statusline_shows_an_active_mandate_only() {
    let seg = statusline_segment(&MandateState::Active(active(false)), now()).unwrap();
    assert!(seg.starts_with("MANDATE "), "{seg}");
    assert_eq!(statusline_segment(&MandateState::None, now()), None);
    assert_eq!(
        statusline_segment(&MandateState::Stopped(active(false)), now()),
        None
    );
}

// ── Wiring: `aida pr ship` consults the mandate where the floor applied ────

#[test]
fn pr_ship_consults_the_mandate_at_every_hold_gate_and_before_the_merge() {
    let src = include_str!("../pr_cmd.rs");
    let handler = src
        .split("pub(crate) fn pr_ship_handler(")
        .nth(1)
        .expect("pr_ship_handler");
    assert!(handler.contains("floor_authority || mandate_cover.is_ok(),\n            pr_number,"));
    assert!(handler.contains("interactive_tty || mandate_cover.is_ok()"));
    assert!(handler.contains("mandate_cover = mandate_cover_now();"));
    assert!(handler.contains("floor_authority || mandate_release.is_some()"));
    assert!(handler.contains("record_mandate_clearance("));
    let before_merge = handler.split(".merge_change(").next().expect("merge call");
    assert!(
        before_merge.contains("mandate_still_active(&cover.id)"),
        "the mandate is re-read immediately before the merge call"
    );
    let preflight = src
        .split("fn ship_preflight_gates(")
        .nth(1)
        .and_then(|b| b.split("pub(crate) fn pr_ship_handler(").next())
        .unwrap();
    assert!(
        !preflight.contains("has_integrity_floor_authority()"),
        "the preflight takes the hold authority the handler decided"
    );
}

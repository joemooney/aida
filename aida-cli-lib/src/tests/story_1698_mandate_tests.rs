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
        scopes: Default::default(),
        kind: None,
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

fn plain() -> aida_core::Requirement {
    let mut req = aida_core::Requirement::new("Improve a list".into(), "fixture".into());
    req.req_type = aida_core::RequirementType::Task;
    req.execution_mode = Some(aida_core::ExecutionMode::Drive);
    req
}

fn spec(id: &str) -> ShipSpec {
    ShipSpec {
        id: id.into(),
        aliases: Vec::new(),
        named: true,
        shipped: false,
        current: Some(plain()),
        history: Ok(vec![plain()]),
    }
}

fn tagged(id: &str, tag: &str) -> ShipSpec {
    let mut s = spec(id);
    s.current.as_mut().unwrap().tags.insert(tag.into());
    s
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
    ledger: std::result::Result<std::collections::BTreeMap<String, Exclusion>, String>,
    hold: HoldFacts,
    carry_over: Option<std::result::Result<String, String>>,
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
            ledger: Ok(Default::default()),
            hold: HoldFacts::Marker {
                kind: crate::merge_hold::HoldReasonKind::Supervision,
                legacy: false,
            },
            carry_over: None,
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
            ledger: &self.ledger,
            hold: &self.hold,
            carry_over: self.carry_over.clone(),
            changed_paths: Some(&[]),
            protected: crate::mandate_gates::ProtectedPaths::floor_ref(),
            ship_gate: None,
            hold_clear_gate: None,
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
        load_valid(tmp.path(), "joe", now()).0.is_empty(),
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
        state_at(&load_valid(tmp.path(), "joe", now()).0, now()),
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
    assert!(load_valid(tmp.path(), "joe", now()).0.is_empty());
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
        state_at(&load_valid(tmp.path(), "joe", now()).0, now()),
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
    c.specs = vec![ShipSpec {
        id: "TASK-404".into(),
        aliases: Vec::new(),
        named: true,
        shipped: false,
        current: None,
        history: Ok(Vec::new()),
    }];
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
    c.state = state_at(&load_valid(tmp.path(), "joe", now()).0, now());
    assert_eq!(c.decide(), Ok(m.id.clone()));
    let stopped = stop(tmp.path(), &human_ok, "joe", now()).unwrap();
    assert_eq!(stopped.len(), 1);
    c.state = state_at(&load_valid(tmp.path(), "joe", now()).0, now());
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
        Err(MandateRefusal::AlwaysHuman {
            spec: "TASK-3".into(),
            class: "operator-only".into(),
        })
    );
    let mut s4 = spec("TASK-4");
    s4.current.as_mut().unwrap().execution_mode = Some(aida_core::ExecutionMode::Operator);
    c.specs = vec![s4];
    assert_eq!(
        c.decide(),
        Err(MandateRefusal::AlwaysHuman {
            spec: "TASK-4".into(),
            class: "operator-only".into(),
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
    // BUG-2214: the ledger clearance moved into the one shared release
    // sequence, which ship and the train both call. trace:BUG-2214 | ai:claude
    assert!(handler.contains("release_hold_under_mandate("));
    let shared = include_str!("../mandate_ship.rs")
        .split("pub(crate) fn release_hold_under_mandate(")
        .nth(1)
        .expect("release_hold_under_mandate");
    assert!(shared.contains("record_mandate_clearance("));
    let train = include_str!("../pr_train.rs");
    assert!(
        train.contains("release_hold_under_mandate("),
        "the train releases through the shared sequence, not a copy"
    );
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

// ── Ruling: exclusion is STICKY across the spec's store history ────────────

#[test]
fn a_tag_stripped_in_a_later_version_is_still_refused() {
    let mut once_secure = plain();
    once_secure.tags.insert("security".into());
    let mut c = Case::ok();
    c.specs = vec![ShipSpec {
        id: "TASK-5".into(),
        aliases: Vec::new(),
        named: true,
        shipped: false,
        current: Some(plain()),
        history: Ok(vec![once_secure, plain()]),
    }];
    assert!(
        matches!(c.decide(), Err(MandateRefusal::Excluded { ref why, .. }) if why.contains("earlier version")),
        "{:?}",
        c.decide()
    );
    c.state = MandateState::Active(active(true));
    assert!(
        c.decide().is_ok(),
        "--include-supervised still lifts a sensitive class"
    );
}

#[test]
fn a_tag_stripped_by_a_recorded_edit_is_still_refused() {
    let mut current = plain();
    current.history.push(aida_core::HistoryEntry::new(
        "agent".into(),
        vec![aida_core::FieldChange {
            field_name: "tags".into(),
            old_value: "containment, area:x".into(),
            new_value: "area:x".into(),
        }],
    ));
    let mut c = Case::ok();
    c.specs = vec![ShipSpec {
        id: "TASK-6".into(),
        aliases: Vec::new(),
        named: true,
        shipped: false,
        current: Some(current),
        history: Ok(vec![plain()]),
    }];
    assert!(matches!(c.decide(), Err(MandateRefusal::Excluded { .. })));
}

#[test]
fn operator_mode_anywhere_in_history_is_always_refused() {
    let mut once_operator = plain();
    once_operator.execution_mode = Some(aida_core::ExecutionMode::Operator);
    let mut c = Case::ok();
    c.state = MandateState::Active(active(true));
    c.specs = vec![ShipSpec {
        id: "TASK-7".into(),
        aliases: Vec::new(),
        named: true,
        shipped: false,
        current: Some(plain()),
        history: Ok(vec![once_operator]),
    }];
    assert_eq!(
        c.decide(),
        Err(MandateRefusal::AlwaysHuman {
            spec: "TASK-7".into(),
            class: "operator-only".into(),
        })
    );
}

#[test]
fn unreadable_history_is_refused_even_with_include_supervised() {
    let mut c = Case::ok();
    c.state = MandateState::Active(active(true));
    c.specs = vec![ShipSpec {
        id: "TASK-8".into(),
        aliases: Vec::new(),
        named: true,
        shipped: false,
        current: Some(plain()),
        history: Err("git failed".into()),
    }];
    assert!(matches!(
        c.decide(),
        Err(MandateRefusal::HistoryUnreadable { .. })
    ));
}

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// End to end against a real git store: a spec committed with `security`,
/// then re-committed without it, is still excluded; a spec with no stored
/// version at all is unreadable.
#[test]
fn store_git_history_keeps_a_stripped_tag_visible() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    let rel = aida_core::object_store::relative_object_path("TASK-9").unwrap();
    let path = root.join(&rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut req = plain();
    req.spec_id = Some("TASK-9".into());
    req.tags.insert("security".into());
    std::fs::write(&path, serde_yaml::to_string(&req).unwrap()).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "v1"]);
    req.tags.clear();
    std::fs::write(&path, serde_yaml::to_string(&req).unwrap()).unwrap();
    git(root, &["commit", "-qam", "strip the tag"]);

    let history = store_history_versions(root, "TASK-9").unwrap();
    assert_eq!(history.len(), 2);
    let mut c = Case::ok();
    c.specs = vec![ShipSpec {
        id: "TASK-9".into(),
        aliases: Vec::new(),
        named: true,
        shipped: false,
        current: Some(req),
        history: Ok(history),
    }];
    assert!(matches!(c.decide(), Err(MandateRefusal::Excluded { .. })));

    assert!(store_history_versions(root, "TASK-10").is_err());
    let not_a_repo = tempfile::tempdir().unwrap();
    assert!(store_history_versions(not_a_repo.path(), "TASK-9").is_err());
}

// ── Review B1: every record is validated on load; revocation is monotonic ──

/// Write `m` as a mandate record the way an attacker (or a buggy tool)
/// might: plain `std::fs::write`, then the given mode.
#[cfg(unix)]
fn plant(dir: &Path, m: &Mandate, mode: u32) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{}.json", m.id));
    std::fs::write(&path, serde_json::to_vec(m).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn a_forged_long_ttl_record_is_ignored_and_never_shadows_a_valid_one() {
    let tmp = tempfile::tempdir().unwrap();
    let valid = start(
        tmp.path(),
        &human_ok,
        "joe",
        Duration::hours(2),
        None,
        false,
        now(),
    )
    .unwrap();
    let mut forged = active(true);
    forged.id = "6f1c1d6e-0000-4000-8000-0000000000ff".into();
    forged.issued_at = now();
    forged.expires_at = now() + Duration::days(30);
    plant(tmp.path(), &forged, 0o600);
    let (mandates, warnings) = load_valid(tmp.path(), "joe", now());
    assert_eq!(mandates, vec![valid.clone()]);
    assert!(
        warnings.iter().any(|w| w.contains("24h cap")),
        "{warnings:?}"
    );
    assert_eq!(state_at(&mandates, now()), MandateState::Active(valid));
}

#[cfg(unix)]
#[test]
fn a_record_issued_in_the_future_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let mut m = active(false);
    m.issued_at = now() + Duration::hours(1);
    m.expires_at = now() + Duration::hours(2);
    plant(tmp.path(), &m, 0o600);
    let (mandates, warnings) = load_valid(tmp.path(), "joe", now());
    assert!(mandates.is_empty());
    assert!(warnings[0].contains("future"), "{warnings:?}");
    // Within the 5-minute skew is fine.
    m.issued_at = now() + Duration::minutes(4);
    plant(tmp.path(), &m, 0o600);
    assert_eq!(load_valid(tmp.path(), "joe", now()).0.len(), 1);
}

#[cfg(unix)]
#[test]
fn records_for_another_principal_seat_or_scope_are_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    plant(tmp.path(), &active(false), 0o600);
    assert!(load_valid(tmp.path(), "mallory", now()).0.is_empty());
    let mut m = active(false);
    m.seat = "reviewer".into();
    plant(tmp.path(), &m, 0o600);
    assert!(load_valid(tmp.path(), "joe", now()).0.is_empty());
    m.seat = MANDATE_SEAT.into();
    m.scope = "ship-everything".into();
    plant(tmp.path(), &m, 0o600);
    assert!(load_valid(tmp.path(), "joe", now()).0.is_empty());
}

#[cfg(unix)]
#[test]
fn a_world_readable_or_symlinked_record_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let path = plant(tmp.path(), &active(false), 0o644);
    let (mandates, warnings) = load_valid(tmp.path(), "joe", now());
    assert!(mandates.is_empty());
    assert!(warnings[0].contains("mode 644"), "{warnings:?}");

    std::fs::remove_file(&path).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let target = plant(elsewhere.path(), &active(false), 0o600);
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let (mandates, warnings) = load_valid(tmp.path(), "joe", now());
    assert!(mandates.is_empty());
    assert!(warnings[0].contains("symlink"), "{warnings:?}");
}

#[cfg(unix)]
#[test]
fn start_writes_a_0600_record() {
    use std::os::unix::fs::PermissionsExt;
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
    let mode = std::fs::metadata(tmp.path().join(format!("{}.json", m.id)))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[cfg(unix)]
#[test]
fn a_stopped_mandate_stays_stopped_when_revoked_at_is_removed() {
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
    stop(tmp.path(), &human_ok, "joe", now()).unwrap();
    // Rewrite the record with no revocation at all: the tombstone still wins.
    let mut revived = m.clone();
    revived.revoked_at = None;
    revived.revoked_by = None;
    plant(tmp.path(), &revived, 0o600);
    let state = state_at(&load_valid(tmp.path(), "joe", now()).0, now());
    assert!(
        matches!(state, MandateState::Stopped(ref s) if s.id == m.id),
        "{state:?}"
    );
    // And a fresh start is still possible.
    assert!(start(
        tmp.path(),
        &human_ok,
        "joe",
        DEFAULT_TTL,
        None,
        false,
        now()
    )
    .is_ok());
}

// ── Review B3: the append-only exclusion ledger ────────────────────────────

#[test]
fn a_spec_in_the_ledger_is_refused_after_its_tag_is_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut secure = plain();
    secure.tags.insert("security".into());
    note_exclusions(tmp.path(), "TASK-11", [&secure]);
    let ledger = read_exclusions(tmp.path());
    assert!(ledger.as_ref().unwrap().contains_key("TASK-11"));
    // Now every version anywhere is clean: the ledger still excludes it.
    let mut c = Case::ok();
    c.specs = vec![spec("TASK-11")];
    c.ledger = ledger;
    assert!(
        matches!(c.decide(), Err(MandateRefusal::Excluded { .. })),
        "{:?}",
        c.decide()
    );
    c.state = MandateState::Active(active(true));
    assert!(
        c.decide().is_ok(),
        "a sensitive class is lifted by --include-supervised"
    );
}

#[test]
fn an_always_human_ledger_entry_is_never_lifted() {
    let tmp = tempfile::tempdir().unwrap();
    let mut decide = plain();
    decide.execution_mode = Some(aida_core::ExecutionMode::Decide);
    note_exclusions(tmp.path(), "TASK-12", [&decide]);
    let mut c = Case::ok();
    c.state = MandateState::Active(active(true));
    c.specs = vec![spec("TASK-12")];
    c.ledger = read_exclusions(tmp.path());
    assert!(matches!(
        c.decide(),
        Err(MandateRefusal::AlwaysHuman { .. })
    ));
}

#[test]
fn the_ledger_is_append_only_and_an_untrustworthy_one_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let mut secure = plain();
    secure.tags.insert("security".into());
    note_exclusions(tmp.path(), "TASK-13", [&secure]);
    note_exclusions(tmp.path(), "TASK-13", [&secure]);
    let text = std::fs::read_to_string(tmp.path().join("exclusions.jsonl")).unwrap();
    assert_eq!(text.lines().count(), 1, "a repeat sighting appends nothing");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(tmp.path().join("exclusions.jsonl"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    std::fs::write(
        tmp.path().join("exclusions.jsonl"),
        format!("{text}{{not json\n"), // }}
    )
    .unwrap();
    let mut c = Case::ok();
    c.ledger = read_exclusions(tmp.path());
    assert!(matches!(
        c.decide(),
        Err(MandateRefusal::LedgerUnreadable(_))
    ));
}

// ── Review B4: only a typed, releasable marker; re-arm on every failure ────

#[test]
fn a_missing_or_untyped_marker_is_refused() {
    let mut c = Case::ok();
    c.hold = HoldFacts::None;
    assert_eq!(c.decide(), Err(MandateRefusal::NoHoldMarker));
    c.hold = HoldFacts::of(None, Some(crate::merge_hold::MARKER_MISSING_REASON));
    assert_eq!(c.decide(), Err(MandateRefusal::MarkerMissing));
    c.hold = HoldFacts::Marker {
        kind: crate::merge_hold::HoldReasonKind::Supervision,
        legacy: true,
    };
    assert!(matches!(
        c.decide(),
        Err(MandateRefusal::HoldNotReleasable { .. })
    ));
    for kind in [
        crate::merge_hold::HoldReasonKind::Recusal,
        crate::merge_hold::HoldReasonKind::Unknown,
    ] {
        c.hold = HoldFacts::Marker {
            kind,
            legacy: false,
        };
        assert!(matches!(
            c.decide(),
            Err(MandateRefusal::HoldNotReleasable { .. })
        ));
    }
    for kind in [
        crate::merge_hold::HoldReasonKind::Decision,
        crate::merge_hold::HoldReasonKind::Rework,
    ] {
        c.hold = HoldFacts::Marker {
            kind,
            legacy: false,
        };
        assert!(c.decide().is_ok(), "{kind:?}");
    }
}

#[test]
fn a_mandate_released_hold_is_re_armed_after_a_merge_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let hold = crate::merge_hold::typed_hold(
        44,
        crate::merge_hold::HoldReasonKind::Supervision,
        "operator review",
        None,
    );
    crate::merge_hold::write_typed_hold(tmp.path(), &hold).unwrap();
    crate::merge_hold::clear_hold(tmp.path(), 44).unwrap();
    assert!(crate::merge_hold::read_hold_record(tmp.path(), 44).is_none());
    crate::pr_cmd::rearm_mandate_hold_with(tmp.path(), &hold, || Ok(())).unwrap();
    let back = crate::merge_hold::read_hold_record(tmp.path(), 44).expect("re-armed");
    assert_eq!(
        back.reason_kind,
        crate::merge_hold::HoldReasonKind::Supervision
    );
    // A label that cannot be restored is reported, never swallowed.
    let err = crate::pr_cmd::rearm_mandate_hold_with(tmp.path(), &hold, || Err("gh down".into()))
        .unwrap_err();
    assert!(err.contains("gh down"), "{err}");
}

#[test]
fn every_failure_after_a_mandate_release_re_arms_the_hold() {
    let src = include_str!("../pr_cmd.rs");
    let handler = src.split("pub(crate) fn pr_ship_handler(").nth(1).unwrap();
    let release = handler
        .find("crate::merge_hold::clear_hold(&hold_root, pr_number)")
        .unwrap();
    let after = &handler[release..];
    let merged = after.find("merged_this_run = true;").unwrap();
    let window = &after[..merged + 2000];
    assert!(window.contains("if mandate_release.is_some() {")); // }
    assert!(window.contains("ended before the merge"));
    assert!(window.contains("re-arm a mandate-released hold on a failed merge"));
    assert_eq!(
        window.matches("rearm_after_mandate_failure(").count(),
        4,
        "gate timeout, lapse, a moved base and merge failure each re-arm"
    );
    // TASK-1915: the moved-base (TASK-1960) refusal re-arms too, through the
    // decision `refusal_with_mandate_rearm_*` tests pin behaviourally.
    // trace:TASK-1915 | ai:claude
    // trace:TASK-1955 | ai:claude (TASK-2088)
    // The call passes exactly the facts the merge-failure arm tests (this
    // ship's mandate released it, and a hold marker was read) in signature
    // order, plus the shared re-arm; a swapped or constant argument fails.
    let flat: String = window.split_whitespace().collect::<Vec<_>>().join(" ");
    let call = concat!(
        "refusal_with_mandate_",
        "rearm( refusal, mandate_release.is_some(), marker_reason.is_some(), ",
        "|| rearm_after_mandate_failure(&hold_root, hold_record.as_ref(), pr_number), );"
    );
    assert_eq!(
        flat.matches(call).count(),
        1,
        "moved-base call site: {call}"
    );
    assert!(flat.contains("if mandate_release.is_some() && marker_reason.is_some() {"));
    // }
}

/// TASK-2088: the moved-base exit re-arms exactly when this ship's mandate
/// released a hold, and never otherwise.
// trace:TASK-1955 | ai:claude (TASK-2088)
#[test]
fn refusal_with_mandate_rearm_re_arms_only_a_mandate_released_hold() {
    use crate::pr_cmd::refusal_with_mandate_rearm;
    let rearmed = refusal_with_mandate_rearm("base moved.".into(), true, true, || {
        " The hold is re-armed.".into()
    });
    assert_eq!(rearmed, "base moved. The hold is re-armed.");
    for (mandate, held) in [(true, false), (false, true), (false, false)] {
        assert_eq!(
            refusal_with_mandate_rearm("base moved.".into(), mandate, held, || {
                panic!("({mandate}, {held}) must not re-arm")
            }),
            "base moved."
        );
    }
}

// ── Review B5: decide mode is always human ─────────────────────────────────

#[test]
fn decide_mode_is_refused_even_with_include_supervised() {
    let mut c = Case::ok();
    c.state = MandateState::Active(active(true));
    let mut s = spec("TASK-14");
    s.current.as_mut().unwrap().execution_mode = Some(aida_core::ExecutionMode::Decide);
    c.specs = vec![s];
    assert_eq!(
        c.decide(),
        Err(MandateRefusal::AlwaysHuman {
            spec: "TASK-14".into(),
            class: "decide-mode".into(),
        })
    );
}

// trace:TASK-1790 | ai:codex
#[test]
fn task_1790_exclusions_require_private_owned_regular_file() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(EXCLUSION_LEDGER);
    assert!(read_exclusions(tmp.path()).unwrap().is_empty());
    write_private(
        &path,
        &serde_json::to_vec(&Exclusion {
            spec: "TASK-13".into(),
            class: "security".into(),
            always_human: false,
            first_seen: now(),
        })
        .unwrap(),
    )
    .unwrap();
    assert!(read_exclusions(tmp.path()).unwrap().contains_key("TASK-13"));
    let meta = path.metadata().unwrap();
    assert!(exclusion_metadata_check(&meta, meta.uid()).is_ok());
    assert!(exclusion_metadata_check(&meta, meta.uid().wrapping_add(1))
        .unwrap_err()
        .contains("owned by uid"));
    for mode in [0o644, 0o660, 0o400, 0o4600] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        let ledger = read_exclusions(tmp.path());
        assert!(
            ledger.as_ref().unwrap_err().contains("not 600"),
            "{mode:o}: {ledger:?}"
        );
        let mut c = Case::ok();
        c.ledger = ledger;
        assert!(matches!(
            c.decide(),
            Err(MandateRefusal::LedgerUnreadable(_))
        ));
    }
}

// trace:TASK-1790 | ai:codex
#[test]
fn task_1790_exclusion_special_files_refuse_without_blocking() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let (done, stopped) = std::sync::mpsc::channel();
    let watchdog = std::thread::spawn(move || {
        if matches!(
            stopped.recv_timeout(std::time::Duration::from_secs(10)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ) {
            std::process::exit(124);
        }
    });
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(EXCLUSION_LEDGER);
    let target = tmp.path().join("private");
    write_private(&target, b"").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(read_exclusions(tmp.path()).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(read_exclusions(tmp.path())
        .unwrap_err()
        .contains("not a regular file"));
    std::fs::remove_dir(&path).unwrap();
    use std::os::unix::ffi::OsStrExt;
    let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(read_exclusions(tmp.path())
        .unwrap_err()
        .contains("not a regular file"));
    done.send(()).unwrap();
    watchdog.join().unwrap();
}

// trace:TASK-1790 | ai:codex
#[test]
fn task_1790_mandate_human_errors_use_mandate_commands() {
    let tmp = tempfile::tempdir().unwrap();
    for (tty, agent, permitted) in [
        (false, None, false),
        (true, Some("fixture-agent"), false),
        (true, None, true),
    ] {
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            tmp.path(),
            "advisor",
            &[],
            &[("AIDA_AGENT_TYPE", None), ("AIDA_AGENT_NAME", agent)],
        );
        // AmbientGuard restores the previous thread-local value on drop.
        crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
            project_root: tmp.path().to_owned(),
            stdin_is_terminal: tty,
            stdout_is_terminal: tty,
            stderr_is_terminal: tty,
        }));
        let result = require_mandate_human();
        assert_eq!(result.is_ok(), permitted);
        if let Err(err) = result {
            let text = err.to_string();
            assert!(text.contains("mandates require a direct human"), "{text}");
            assert!(text.contains("aida mandate start") && text.contains("aida mandate stop"));
            assert!(!text.contains("role grant"));
        }
    }
}

// trace:TASK-1790 | ai:codex
#[test]
fn task_1790_rearm_audit_follows_release_before_label_and_keeps_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        tmp.path(),
        "advisor",
        &[],
        &[("AIDA_EVENTS_DISABLE", None)],
    );
    let hold = crate::merge_hold::typed_hold(
        44,
        crate::merge_hold::HoldReasonKind::Supervision,
        "operator review",
        None,
    );
    crate::merge_hold::write_typed_hold(tmp.path(), &hold).unwrap();
    crate::merge_hold::clear_hold(tmp.path(), 44).unwrap();
    crate::merge_hold::record_mandate_clearance(tmp.path(), &hold, "joe", "M-1", None).unwrap();
    // Same release event emitted by pr_ship_handler after clearance.
    crate::events::emit(
        tmp.path(),
        &crate::events::Event::new(
            None,
            "",
            crate::events::EventKind::MergeHoldChanged {
                pr: 44,
                placed: false,
                reason: Some("released under mandate M-1".into()),
                principal: None,
                human_attested: false,
            },
        ),
    );
    let before = crate::events::read_all(tmp.path()).len();
    let err = crate::pr_cmd::rearm_mandate_hold_with(tmp.path(), &hold, || {
        let events = crate::events::read_all(tmp.path());
        assert_eq!(events.len(), before + 1, "one placement event, no duplicate audit");
        assert!(matches!(&events[before - 1].kind, crate::events::EventKind::MergeHoldChanged { placed: false, .. }));
        assert!(matches!(&events[before].kind, crate::events::EventKind::MergeHoldChanged { pr: 44, placed: true, reason: Some(reason), .. } if reason == "Re-armed after mandate ship failure: operator review"));
        let restored = crate::merge_hold::read_hold_record(tmp.path(), 44).unwrap();
        assert_eq!(restored.detail, hold.detail);
        assert_eq!(restored.reason_kind, hold.reason_kind);
        Err("label unavailable".into())
    }).unwrap_err();
    assert!(err.contains("label unavailable"));
}

// trace:TASK-1790 | ai:codex
#[test]
fn task_1790_failed_marker_rearm_emits_no_placement_event() {
    let tmp = tempfile::tempdir().unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        tmp.path(),
        "advisor",
        &[],
        &[("AIDA_EVENTS_DISABLE", None)],
    );
    let hold = crate::merge_hold::typed_hold(
        45,
        crate::merge_hold::HoldReasonKind::Supervision,
        "operator review",
        None,
    );
    let path = crate::merge_hold::hold_path(tmp.path(), 45);
    std::fs::create_dir_all(&path).unwrap();
    let before = crate::events::read_all(tmp.path()).len();
    let mut label_tried = false;
    let err = crate::pr_cmd::rearm_mandate_hold_with(tmp.path(), &hold, || {
        label_tried = true;
        Err("label unavailable".into())
    })
    .unwrap_err();
    assert!(label_tried);
    assert!(err.contains("marker could not be rewritten") && err.contains("label unavailable"));
    assert_eq!(crate::events::read_all(tmp.path()).len(), before);
}

// trace:TASK-1790 | ai:codex
#[test]
fn additional_trace_spec_can_restrict_but_cannot_supply_ship_authority() {
    let mut c = Case::ok();
    assert!(c.decide().is_ok());
    c.specs.push(tagged("TASK-2", "operator-only"));
    assert!(
        matches!(c.decide(), Err(MandateRefusal::AlwaysHuman { spec, .. }) if spec == "TASK-2")
    );

    let mut c = Case::ok();
    c.specs.push(spec("TASK-2"));
    c.specs_resolved = false; // No resolved named subject, even with trace IDs.
    assert_eq!(c.decide(), Err(MandateRefusal::SpecsUnresolved));

    let mut c = Case::ok();
    c.candidates.clear();
    let before = c.decide();
    assert!(before.is_err());
    c.specs.push(spec("TASK-2"));
    assert_eq!(c.decide(), before, "extra exclusions supply no approval");

    let mut c = Case::ok();
    c.candidates = vec![approval_by(HEAD, ADVISOR)];
    c.policy = policy(&c.candidates);
    assert_eq!(c.decide(), Err(MandateRefusal::ApproverIsShipper));
    c.specs.push(spec("TASK-2"));
    assert_eq!(c.decide(), Err(MandateRefusal::ApproverIsShipper));
}

// trace:TASK-1790 | ai:codex
#[test]
fn named_union_trace_assembly_refuses_trace_only_supervised_spec() {
    let mut c = Case::ok();
    let named = vec!["TASK-1".to_owned()];
    let traced = ["TASK-2".to_owned()].into_iter().collect();
    c.specs = crate::pr_cmd::mandate_exclusion_specs(&named, traced, |id, is_named| {
        let mut value = spec(id);
        value.named = is_named;
        if id == "TASK-2" {
            value.current.as_mut().unwrap().execution_mode = Some(aida_core::ExecutionMode::Guided);
        }
        value
    });
    assert!(matches!(c.decide(), Err(MandateRefusal::Excluded { spec, .. }) if spec == "TASK-2"));
}

// trace:TASK-1790 | ai:codex
#[test]
fn exclusion_writer_refuses_invalid_ledger_without_appending() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(EXCLUSION_LEDGER);
    let seen = vec![("TASK-2".into(), "guided".into(), false)];
    record_exclusions(tmp.path(), &seen, now()).unwrap();
    let before = std::fs::read(&path).unwrap();
    for mode in [0o644, 0o4600] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(private_file_check(&path).is_err());
        assert!(record_exclusions(
            tmp.path(),
            &[("TASK-3".into(), "guided".into(), false)],
            now()
        )
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&path, b"malformed\n").unwrap();
    assert!(record_exclusions(tmp.path(), &seen, now()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"malformed\n");
}

// trace:TASK-1790 | ai:codex
#[test]
fn exclusion_writer_fifo_refuses_within_deadline() {
    use std::os::unix::ffi::OsStrExt;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(EXCLUSION_LEDGER);
    let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = record_exclusions(
            tmp.path(),
            &[("TASK-3".into(), "guided".into(), false)],
            now(),
        );
        tx.send(result.is_err()).unwrap();
    });
    assert!(rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("FIFO writer blocked"));
    worker.join().unwrap();
}

// Exercise the real mandate_ship_cover wiring, not just the union helper. An
// empty-set mutation at its trace_evidence.ids argument must fail this test.
// trace:TASK-1800 | ai:codex
#[test]
fn mandate_ship_cover_passes_committed_trace_ids_to_the_decision() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let git = |dir: &Path, args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(root, &["init", "-q"]);
    git(
        root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/fixture/project.git",
        ],
    );
    std::fs::write(
        root.join("code.rs"),
        "//! trace:SUP-1\nuse std::fmt;\n// trace:MODULE-2\nconst MARKER: u8 = 0;\n// trace:ITEM-3\nfn changed() { let x = 1; }\n",
    )
    .unwrap();
    git(root, &["add", "code.rs"]);
    git(root, &["commit", "-qm", "base"]);
    let base = git(root, &["rev-parse", "HEAD"]);
    git(root, &["update-ref", "refs/remotes/origin/main", &base]);
    std::fs::write(
        root.join("code.rs"),
        "//! trace:SUP-1\nuse std::fmt;\n// trace:MODULE-2\nconst MARKER: u8 = 0;\n// trace:ITEM-3\nfn changed() { let x = 2; }\n",
    )
    .unwrap();
    git(root, &["add", "code.rs"]);
    git(root, &["commit", "-qm", "change (TASK-1)"]);
    let head = git(root, &["rev-parse", "HEAD"]);
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "store_path = '.aida-store'\n",
    )
    .unwrap();
    let store = root.join(".aida-store");
    std::fs::create_dir(&store).unwrap();
    git(&store, &["init", "-q"]);
    git(
        &store,
        &["commit", "--allow-empty", "-qm", "empty fixture store"],
    );
    let gh = root.join("gh-stub");
    crate::test_exec::write_executable(&gh, format!("#!/bin/sh\nprintf '%s\\n' '{{\"baseRefName\":\"main\",\"headRefName\":\"feature\",\"headRefOid\":\"{head}\"}}'\n"));
    let home = root.join("home");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        root,
        "advisor",
        &[],
        &[
            ("AIDA_HOME", Some(home.to_str().unwrap())),
            ("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap())),
        ],
    );
    // The seat fixture seeds registry/team.toml in the canonical store. Commit
    // that setup before exercising the production clean-store boundary.
    git(&store, &["add", "-A"]);
    git(&store, &["commit", "-qm", "seed fixture seat roster"]);
    assert!(crate::mandate_traces::store_snapshot(&store).is_ok());
    let mut mandate = active(false);
    mandate.principal = crate::current_user_id(None);
    mandate.issued_at = Utc::now() - Duration::minutes(1);
    mandate.expires_at = Utc::now() + Duration::hours(1);
    plant(&mandate_dir().unwrap(), &mandate, 0o600);
    let result = crate::with_review_pr_subject(
        crate::ReviewPrSubject {
            head_ref: "feature".into(),
            commit_subjects: vec!["change (TASK-1)".into()],
        },
        || {
            crate::pr_cmd::mandate_ship_cover(
                root,
                root,
                root,
                42,
                &["TASK-1".into()],
                Some("feature"),
                || Some(head.clone()),
                false,
                |check| {
                    assert!(
                        check.specs_resolved,
                        "fixture must reach the resolved decision"
                    );
                    assert_eq!(
                        check
                            .specs
                            .iter()
                            .map(|s| (s.id.as_str(), s.named))
                            .collect::<Vec<_>>(),
                        vec![
                            ("ITEM-3", false),
                            ("MODULE-2", false),
                            ("SUP-1", false),
                            ("TASK-1", true)
                        ]
                    );
                    // No approval/clearance is fabricated: the fixture decision refuses.
                    Err(MandateRefusal::SpecsUnresolved)
                },
                false,
            )
        },
    );
    assert!(
        matches!(result, Err(MandateRefusal::SpecsUnresolved)),
        "{result:?}"
    );
}

// ── STORY-1700 × STORY-1655: a solo self-approval is not independent ──────

// trace:STORY-1700.ac8eabb8 | ai:claude
#[test]
fn a_solo_self_approval_never_covers_a_mandate_ship_or_release() {
    let mut c = Case::ok();
    let mut v = approval(HEAD);
    v.attestation.as_mut().unwrap().solo_human = true;
    c.candidates = vec![v.clone()];
    c.policy = policy(&c.candidates);
    // Solo mode off (or the receipt unverified): an ordinary human approval.
    assert!(c.decide().is_ok(), "{:?}", c.decide());
    // Solo mode on and the solo receipt verified: the self-approval may pass
    // the author check by hand, but never a mandate.
    let key = v
        .attestation
        .as_ref()
        .unwrap()
        .authority_key(v.recorded_at.as_deref())
        .unwrap();
    assert!(key.starts_with("receipt:"), "{key}");
    c.policy.solo_human = true;
    c.policy.verified_solo.insert(key);
    assert!(crate::pr_ship::solo_self_approval(&v, &c.policy));
    assert_eq!(c.decide(), Err(MandateRefusal::SoloSelfApproval));
    // The rework-release path refuses it too.
    c.state = MandateState::Active(active(true));
    c.hold = HoldFacts::Marker {
        kind: crate::merge_hold::HoldReasonKind::Rework,
        legacy: false,
    };
    let release = rework_release_decision(&ShipCheck {
        state: &c.state,
        now: c.now,
        grant: c.grant,
        specs: &c.specs,
        specs_resolved: c.specs_resolved,
        candidates: &c.candidates,
        policy: &c.policy,
        head: c.head,
        override_stale_approval: false,
        ledger: &c.ledger,
        hold: &c.hold,
        carry_over: None,
        changed_paths: Some(&[]),
        protected: crate::mandate_gates::ProtectedPaths::floor_ref(),
        ship_gate: None,
        hold_clear_gate: None,
    });
    assert_eq!(release, Err(MandateRefusal::SoloSelfApproval));
}

// ── TASK-1800 advisor ruling (2026-10-17) ──────────────────────────────────

// trace:TASK-1800 | ai:antigravity
#[test]
fn task_1800_dangling_trace_id_is_allowed_while_named_unknown_still_refuses() {
    let mut c = Case::ok();
    // A trace-derived ID with no current version, no store history on any ref,
    // and no exclusion ledger entry is a dangling reference and is dropped.
    c.specs.push(ShipSpec {
        id: "EPIC-1-001".into(),
        aliases: Vec::new(),
        named: false,
        shipped: false,
        current: None,
        history: Ok(Vec::new()),
    });
    assert_eq!(c.decide(), Ok(active(false).id));

    // A named ID with no current version still fails closed with SpecsUnresolved.
    let mut c = Case::ok();
    c.specs.push(ShipSpec {
        id: "TASK-404".into(),
        aliases: Vec::new(),
        named: true,
        shipped: false,
        current: None,
        history: Ok(Vec::new()),
    });
    assert_eq!(c.decide(), Err(MandateRefusal::SpecsUnresolved));

    // Unreadable history on a trace-derived ID still fails closed.
    let mut c = Case::ok();
    c.specs.push(ShipSpec {
        id: "EPIC-1-001".into(),
        aliases: Vec::new(),
        named: false,
        shipped: false,
        current: None,
        history: Err("git log failed".into()),
    });
    assert!(matches!(
        c.decide(),
        Err(MandateRefusal::HistoryUnreadable { ref spec, .. }) if spec == "EPIC-1-001"
    ));

    // A trace-only list with no named spec still fails closed.
    let mut c = Case::ok();
    c.specs = vec![ShipSpec {
        id: "TASK-2".into(),
        aliases: Vec::new(),
        named: false,
        shipped: false,
        current: Some(plain()),
        history: Ok(vec![plain()]),
    }];
    assert_eq!(c.decide(), Err(MandateRefusal::SpecsUnresolved));
}

// trace:TASK-1800 | ai:antigravity
#[test]
fn task_1800_deleted_spec_with_history_is_judged_by_history() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    let rel = aida_core::object_store::relative_object_path("TASK-99").unwrap();
    let path = root.join(&rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut req = plain();
    req.spec_id = Some("TASK-99".into());
    req.tags.insert("security".into());
    std::fs::write(&path, serde_yaml::to_string(&req).unwrap()).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "add excluded spec"]);
    std::fs::remove_file(&path).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "delete spec"]);

    let history = store_history_versions_optional(root, "TASK-99").unwrap();
    assert_eq!(history.len(), 1);
    let mut c = Case::ok();
    c.specs.push(ShipSpec {
        id: "TASK-99".into(),
        aliases: Vec::new(),
        named: false,
        shipped: false,
        current: None,
        history: Ok(history),
    });
    assert!(
        matches!(c.decide(), Err(MandateRefusal::Excluded { ref spec, ref why }) if spec == "TASK-99" && why.contains("earlier version")),
        "{:?}",
        c.decide()
    );
    c.state = MandateState::Active(active(true));
    assert_eq!(c.decide(), Ok(active(true).id));
}

// trace:TASK-1800 | ai:antigravity
#[test]
fn task_1800_aliased_id_resolves_through_alias_and_is_judged_normally() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let git_out = |dir: &Path, args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git_out(root, &["init", "-q"]);
    git_out(
        root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/fixture/project.git",
        ],
    );
    // Trace comment uses agreed_id TASK-176 (stored under origin spec_id TASK-1-152)
    // plus a dangling trace FR-0042 that never existed in the store.
    std::fs::write(
        root.join("code.rs"),
        "//! trace:TASK-176\n//! trace:FR-0042\nfn changed() { let x = 1; }\n",
    )
    .unwrap();
    git_out(root, &["add", "code.rs"]);
    git_out(root, &["commit", "-qm", "base"]);
    let base = git_out(root, &["rev-parse", "HEAD"]);
    git_out(root, &["update-ref", "refs/remotes/origin/main", &base]);
    std::fs::write(
        root.join("code.rs"),
        "//! trace:TASK-176\n//! trace:FR-0042\nfn changed() { let x = 2; }\n",
    )
    .unwrap();
    git_out(root, &["add", "code.rs"]);
    git_out(root, &["commit", "-qm", "change (TASK-1)"]);
    let head = git_out(root, &["rev-parse", "HEAD"]);
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "store_path = '.aida-store'\n",
    )
    .unwrap();
    let store = root.join(".aida-store");
    std::fs::create_dir(&store).unwrap();
    git_out(&store, &["init", "-q"]);

    // Seed named TASK-1 (ordinary) and aliased TASK-1-152 (agreed_id = TASK-176).
    // First version of TASK-1-152 had `security` tag in history, stripped in v2.
    let rel1 = aida_core::object_store::relative_object_path("TASK-1").unwrap();
    let path1 = store.join(&rel1);
    std::fs::create_dir_all(path1.parent().unwrap()).unwrap();
    let mut req1 = plain();
    req1.spec_id = Some("TASK-1".into());
    std::fs::write(&path1, serde_yaml::to_string(&req1).unwrap()).unwrap();

    let rel_alias = aida_core::object_store::relative_object_path("TASK-1-152").unwrap();
    let path_alias = store.join(&rel_alias);
    std::fs::create_dir_all(path_alias.parent().unwrap()).unwrap();
    let mut req_alias = plain();
    req_alias.spec_id = Some("TASK-1-152".into());
    req_alias.agreed_id = Some("TASK-176".into());
    req_alias.tags.insert("security".into());
    std::fs::write(&path_alias, serde_yaml::to_string(&req_alias).unwrap()).unwrap();
    git_out(&store, &["add", "-A"]);
    git_out(&store, &["commit", "-qm", "v1 store"]);

    // Strip the security tag on TASK-1-152 so only its store history (under TASK-1-152) holds it.
    req_alias.tags.clear();
    std::fs::write(&path_alias, serde_yaml::to_string(&req_alias).unwrap()).unwrap();
    git_out(&store, &["add", "-A"]);
    git_out(&store, &["commit", "-qm", "v2 strip tag"]);

    let gh = root.join("gh-stub");
    crate::test_exec::write_executable(&gh, format!("#!/bin/sh\nprintf '%s\\n' '{{\"baseRefName\":\"main\",\"headRefName\":\"feature\",\"headRefOid\":\"{head}\"}}'\n"));
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        root,
        "advisor",
        &[],
        &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
    );
    git_out(&store, &["add", "-A"]);
    git_out(
        &store,
        &["commit", "--allow-empty", "-qm", "seed fixture seat roster"],
    );

    let mut mandate = active(false);
    mandate.principal = crate::current_user_id(None);
    mandate.issued_at = Utc::now() - Duration::minutes(1);
    mandate.expires_at = Utc::now() + Duration::hours(1);
    plant(&mandate_dir().unwrap(), &mandate, 0o600);

    let hold = crate::merge_hold::typed_hold(
        42,
        crate::merge_hold::HoldReasonKind::Supervision,
        "operator review",
        None,
    );
    crate::merge_hold::write_typed_hold(root, &hold).unwrap();

    // Run mandate_ship_cover and verify that:
    // 1. Dangling FR-0042 does not cause HistoryUnreadable or SpecsUnresolved.
    // 2. Aliased TASK-176 resolves its history via TASK-1-152 and is refused as Excluded!
    let result = crate::with_review_pr_subject(
        crate::ReviewPrSubject {
            head_ref: "feature".into(),
            commit_subjects: vec!["change (TASK-1)".into()],
        },
        || {
            crate::pr_cmd::mandate_ship_cover(
                root,
                root,
                root,
                42,
                &["TASK-1".into()],
                Some("feature"),
                || Some(head.clone()),
                false,
                crate::mandate::ship_decision,
                true,
            )
        },
    );
    assert!(
        matches!(result, Err(MandateRefusal::Excluded { ref spec, .. }) if spec == "TASK-176"),
        "expected Excluded for aliased TASK-176 via TASK-1-152 history, got: {result:?}"
    );
}

// trace:TASK-1800 | ai:claude
#[test]
fn task_1800_named_spec_with_empty_history_is_refused_as_unreadable() {
    let mut c = Case::ok();
    c.specs = vec![ShipSpec {
        id: "TASK-9".into(),
        aliases: Vec::new(),
        named: true,
        shipped: false,
        current: Some(plain()),
        history: Ok(Vec::new()),
    }];
    assert!(
        matches!(c.decide(), Err(MandateRefusal::HistoryUnreadable { ref spec, .. }) if spec == "TASK-9"),
        "{:?}",
        c.decide()
    );
}

// trace:TASK-1800 | ai:claude
#[test]
fn task_1800_alias_search_outside_a_store_repository_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(crate::mandate::store_history_alias_ids(tmp.path(), "TASK-176").is_err());
    assert!(crate::mandate::store_history_alias_ids(tmp.path(), "TASK 176;").is_err());
}

/// TASK-1800: ship a change whose trace names TASK-176, the agreed_id of a
/// security-tagged spec TASK-1-152 that was stored at `stored_at` (relative
/// to the store) and then deleted.
// trace:TASK-1800 | ai:claude
fn ship_tracing_deleted_agreed_id_spec(
    stored_at: Option<&str>,
) -> std::result::Result<crate::pr_cmd::MandateCover, MandateRefusal> {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let git_out = |dir: &Path, args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git_out(root, &["init", "-q"]);
    git_out(
        root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/fixture/project.git",
        ],
    );
    // The trace names the deleted spec by its agreed_id TASK-176.
    std::fs::write(
        root.join("code.rs"),
        "//! trace:TASK-176\nfn changed() { let x = 1; }\n",
    )
    .unwrap();
    git_out(root, &["add", "code.rs"]);
    git_out(root, &["commit", "-qm", "base"]);
    let base = git_out(root, &["rev-parse", "HEAD"]);
    git_out(root, &["update-ref", "refs/remotes/origin/main", &base]);
    std::fs::write(
        root.join("code.rs"),
        "//! trace:TASK-176\nfn changed() { let x = 2; }\n",
    )
    .unwrap();
    git_out(root, &["add", "code.rs"]);
    git_out(root, &["commit", "-qm", "change (TASK-1)"]);
    let head = git_out(root, &["rev-parse", "HEAD"]);
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "store_path = '.aida-store'\n",
    )
    .unwrap();
    let store = root.join(".aida-store");
    std::fs::create_dir(&store).unwrap();
    git_out(&store, &["init", "-q"]);

    let rel1 = aida_core::object_store::relative_object_path("TASK-1").unwrap();
    let path1 = store.join(&rel1);
    std::fs::create_dir_all(path1.parent().unwrap()).unwrap();
    let mut req1 = plain();
    req1.spec_id = Some("TASK-1".into());
    std::fs::write(&path1, serde_yaml::to_string(&req1).unwrap()).unwrap();

    // Stored under spec_id TASK-1-152 with agreed_id TASK-176 and an excluded
    // tag, then deleted: only its store history under TASK-1-152 remains.
    let canonical = aida_core::object_store::relative_object_path("TASK-1-152").unwrap();
    let rel_alias = stored_at.unwrap_or(&canonical);
    let path_alias = store.join(rel_alias);
    std::fs::create_dir_all(path_alias.parent().unwrap()).unwrap();
    let mut req_alias = plain();
    req_alias.spec_id = Some("TASK-1-152".into());
    req_alias.agreed_id = Some("TASK-176".into());
    req_alias.tags.insert("security".into());
    std::fs::write(&path_alias, serde_yaml::to_string(&req_alias).unwrap()).unwrap();
    git_out(&store, &["add", "-A"]);
    git_out(&store, &["commit", "-qm", "v1 store"]);
    std::fs::remove_file(&path_alias).unwrap();
    git_out(&store, &["add", "-A"]);
    git_out(&store, &["commit", "-qm", "delete TASK-1-152"]);

    let aliases = crate::mandate::store_history_alias_ids(&store, "TASK-176");
    if stored_at.is_none() {
        assert_eq!(aliases.unwrap(), vec!["TASK-1-152".to_string()]);
    } else {
        assert!(aliases.is_err(), "{aliases:?}");
    }

    let gh = root.join("gh-stub");
    crate::test_exec::write_executable(&gh, format!("#!/bin/sh\nprintf '%s\\n' '{{\"baseRefName\":\"main\",\"headRefName\":\"feature\",\"headRefOid\":\"{head}\"}}'\n"));
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        root,
        "advisor",
        &[],
        &[("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap()))],
    );
    git_out(&store, &["add", "-A"]);
    git_out(
        &store,
        &["commit", "--allow-empty", "-qm", "seed fixture seat roster"],
    );

    let mut mandate = active(false);
    mandate.principal = crate::current_user_id(None);
    mandate.issued_at = Utc::now() - Duration::minutes(1);
    mandate.expires_at = Utc::now() + Duration::hours(1);
    plant(&mandate_dir().unwrap(), &mandate, 0o600);

    let hold = crate::merge_hold::typed_hold(
        42,
        crate::merge_hold::HoldReasonKind::Supervision,
        "operator review",
        None,
    );
    crate::merge_hold::write_typed_hold(root, &hold).unwrap();

    let result = crate::with_review_pr_subject(
        crate::ReviewPrSubject {
            head_ref: "feature".into(),
            commit_subjects: vec!["change (TASK-1)".into()],
        },
        || {
            crate::pr_cmd::mandate_ship_cover(
                root,
                root,
                root,
                42,
                &["TASK-1".into()],
                Some("feature"),
                || Some(head.clone()),
                false,
                crate::mandate::ship_decision,
                true,
            )
        },
    );
    result
}

// trace:TASK-1800 | ai:claude
#[test]
fn task_1800_deleted_spec_named_by_its_agreed_id_is_judged_by_its_history() {
    let result = ship_tracing_deleted_agreed_id_spec(None);
    assert!(
        matches!(result, Err(MandateRefusal::Excluded { ref spec, .. }) if spec == "TASK-176"),
        "a deleted spec named by its agreed_id must be judged, not dropped: {result:?}"
    );
}

/// TASK-1800 comment 24: the history read finds versions only at an
/// object's canonical path. A deleted spec whose history sits anywhere else
/// (here a wrong shard) used to read as no history and be dropped as a
/// dangling trace ID; it must refuse instead.
// trace:TASK-1800 | ai:claude
#[test]
fn task_1800_deleted_agreed_id_spec_at_a_non_canonical_path_refuses() {
    let result = ship_tracing_deleted_agreed_id_spec(Some("objects/TASK/001/TASK-1-152.yaml"));
    assert!(
        matches!(result, Err(MandateRefusal::HistoryUnreadable { ref spec, .. }) if spec == "TASK-176"),
        "a deleted spec named by its agreed_id must never be dropped as dangling: {result:?}"
    );
}

/// TASK-1960: a held PR shipped under an active mandate whose head moved only
/// by a clean main merge proceeds on the carried-over approval; an evil merge
/// is still a stale approval, and the refusal names why.
// trace:TASK-1960 | ai:claude
#[test]
fn mandate_ship_carries_over_a_clean_main_merge_but_not_an_evil_one() {
    use crate::review_carryover::tests as fx;
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let (tmp, approved) = fx::fixture();
    let repo = tmp.path();
    let approved: &'static str = leak(approved);

    let mut case = Case::ok();
    case.candidates = vec![approval(approved)];
    case.policy = policy(&case.candidates);
    let clean = leak(fx::clean_merge(repo));
    case.head = Some(clean);
    // Without a proof the moved head is stale, as before.
    assert!(matches!(
        case.decide(),
        Err(MandateRefusal::StaleApproval(_))
    ));
    case.carry_over =
        crate::pr_cmd::mandate_carry_over(repo, 42, &case.candidates, case.head, Some("main"));
    assert_eq!(case.carry_over, Some(Ok(clean.to_string())));
    assert_eq!(
        case.decide().as_deref(),
        Ok("6f1c1d6e-0000-4000-8000-000000000001"),
        "a held PR ships under the mandate on the carried-over approval"
    );
    // A proof for some other head never covers this one.
    case.carry_over = Some(Ok(approved.to_string()));
    assert!(matches!(
        case.decide(),
        Err(MandateRefusal::StaleApproval(_))
    ));

    let evil = leak(fx::evil_merge(repo, approved));
    case.head = Some(evil);
    case.carry_over =
        crate::pr_cmd::mandate_carry_over(repo, 42, &case.candidates, case.head, Some("main"));
    match case.decide() {
        Err(MandateRefusal::StaleApproval(why)) => {
            assert!(why.contains("did not carry over"), "{why}")
        }
        other => panic!("an evil merge must stay a stale approval: {other:?}"),
    }
    // The mandate path only proves; the pinning gate records.
    assert!(crate::review_carryover::read_record(repo, 42, clean).is_none());
}

// ── BUG-2179: one bounded store-history read for every traced spec ─────────

/// A store repo where TASK-1 had three versions (the middle one excluded),
/// TASK-2 one, TASK-3 was added excluded and then deleted, TASK-5 changed in
/// the same commits, and TASK-6 exists only on a side ref; TASK-4 never existed.
// trace:BUG-2179 | ai:claude
fn bug_2179_store() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    let write = |id: &str, tag: Option<&str>, title: &str| {
        let rel = aida_core::object_store::relative_object_path(id).unwrap();
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut req = plain();
        req.spec_id = Some(id.into());
        req.title = title.into();
        if let Some(tag) = tag {
            req.tags.insert(tag.into());
        }
        std::fs::write(path, serde_yaml::to_string(&req).unwrap()).unwrap();
    };
    let commit = |msg: &str| {
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", msg]);
    };
    write("TASK-1", None, "v1");
    write("TASK-2", None, "only");
    write("TASK-5", None, "noise 1");
    commit("one");
    write("TASK-1", Some("security"), "v2");
    write("TASK-3", Some("security"), "doomed");
    write("TASK-5", None, "noise 2");
    commit("two");
    write("TASK-1", None, "v3");
    std::fs::remove_file(
        root.join(aida_core::object_store::relative_object_path("TASK-3").unwrap()),
    )
    .unwrap();
    commit("three");
    git(root, &["checkout", "-q", "-b", "side"]);
    write("TASK-6", Some("security"), "side only");
    commit("side");
    git(root, &["checkout", "-q", "main"]);
    tmp
}

/// The pre-BUG-2179 algorithm: a full-history walk and one read per version,
/// per spec. The batch must return exactly what it returned.
// trace:BUG-2179 | ai:claude
fn per_spec_reference(root: &Path, id: &str) -> Vec<String> {
    let rel = aida_core::object_store::relative_object_path(id).unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let log = run(&[
        "log",
        "--all",
        "--format=%H",
        "--diff-filter=ACMRT",
        "--",
        &rel,
    ]);
    let mut out = Vec::new();
    for sha in log.lines().filter(|l| !l.trim().is_empty()) {
        let req: aida_core::Requirement =
            serde_yaml::from_str(&run(&["show", &format!("{sha}:{rel}")])).unwrap();
        for v in field_change_versions(&req)
            .iter()
            .chain(std::iter::once(&req))
        {
            out.push(serde_yaml::to_string(v).unwrap());
        }
    }
    out
}

// trace:BUG-2179 | ai:claude
#[test]
fn bug_2179_batched_history_matches_the_per_spec_walk() {
    let store = bug_2179_store();
    let ids: Vec<String> = ["TASK-1", "TASK-2", "TASK-3", "TASK-4", "TASK-6", "task-2"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let batch = StoreHistoryBatch::read(store.path(), &ids).unwrap();
    for id in ["TASK-1", "TASK-2", "TASK-3", "TASK-4", "TASK-6"] {
        let got: Vec<String> = batch
            .versions(id)
            .unwrap()
            .iter()
            .map(|v| serde_yaml::to_string(v).unwrap())
            .collect();
        assert_eq!(got, per_spec_reference(store.path(), id), "{id}");
    }
    assert_eq!(batch.versions("TASK-1").unwrap().len(), 3);
    assert!(batch.versions("TASK-4").unwrap().is_empty());
    assert!(
        batch.versions("TASK-9").is_err(),
        "an ID not read is never `no history`"
    );
    assert!(StoreHistoryBatch::read(tempfile::tempdir().unwrap().path(), &ids).is_err());
}

// trace:BUG-2179 | ai:claude
#[test]
fn bug_2179_batched_history_still_refuses_an_excluded_earlier_version() {
    let store = bug_2179_store();
    let ids = vec![
        "TASK-1".to_string(),
        "TASK-2".to_string(),
        "TASK-6".to_string(),
    ];
    let batch = StoreHistoryBatch::read(store.path(), &ids).unwrap();
    // TASK-1 is plain now; only its middle version carried `security`.
    let mut c = Case::ok();
    c.specs = ids
        .iter()
        .map(|id| ShipSpec {
            id: id.clone(),
            aliases: Vec::new(),
            named: id == "TASK-2",
            shipped: false,
            current: Some(plain()),
            history: batch.versions(id),
        })
        .collect();
    assert!(
        matches!(c.decide(), Err(MandateRefusal::Excluded { ref spec, .. }) if spec == "TASK-1"),
        "{:?}",
        c.decide()
    );
    // Without the excluded spec the same batch ships, so the refusal above
    // came from TASK-1's earlier version and not from the batch itself.
    c.specs.retain(|s| s.id == "TASK-2");
    assert!(c.decide().is_ok(), "{:?}", c.decide());
}

// trace:BUG-2179 | ai:claude
#[test]
fn bug_2179_alias_map_matches_the_per_id_search() {
    let store = bug_2179_store();
    let root = store.path();
    let write = |id: &str, agreed: &str| {
        let rel = aida_core::object_store::relative_object_path(id).unwrap();
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut req = plain();
        req.spec_id = Some(id.into());
        req.agreed_id = Some(agreed.into());
        std::fs::write(path, serde_yaml::to_string(&req).unwrap()).unwrap();
    };
    write("TASK-1-152", "TASK-176");
    write("TASK-1-153", "TASK-177");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "aliases"]);
    for id in ["TASK-1-152", "TASK-1-153"] {
        std::fs::remove_file(root.join(aida_core::object_store::relative_object_path(id).unwrap()))
            .unwrap();
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "delete aliases"]);
    let ids: Vec<String> = ["TASK-176", "task-177", "TASK-3", "TASK-178"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let map = store_history_alias_map(root, &ids).unwrap();
    assert_eq!(map["TASK-176"], vec!["TASK-1-152".to_string()]);
    assert_eq!(map["TASK-177"], vec!["TASK-1-153".to_string()]);
    assert!(map["TASK-3"].is_empty(), "its own object is not an alias");
    assert!(map["TASK-178"].is_empty());
    for id in &ids {
        assert_eq!(
            map[&id.to_ascii_uppercase()],
            store_history_alias_ids(root, id).unwrap(),
            "{id}"
        );
    }
    assert!(store_history_alias_map(root, &["TASK 1;".to_string()]).is_err());
}

// trace:BUG-2179 | ai:claude
#[cfg(unix)]
#[test]
fn bug_2179_history_for_many_specs_runs_a_bounded_number_of_gits() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH")
        .display()
        .to_string();
    let store = bug_2179_store();
    let shim = tempfile::tempdir().unwrap();
    let log = shim.path().join("calls");
    crate::test_exec::write_executable(
        &shim.path().join("git"),
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nexec '{real}' \"$@\"\n",
            log.display()
        ),
    );
    let path = format!(
        "{}:{}",
        shim.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _guard = crate::test_env::EnvVarsGuard::apply(&[("PATH", Some(&path))]);
    let ids: Vec<String> = (1..=40).map(|n| format!("TASK-{n}")).collect();
    let batch = StoreHistoryBatch::read(store.path(), &ids).unwrap();
    assert_eq!(batch.versions("TASK-1").unwrap().len(), 3);
    store_history_alias_map(store.path(), &ids).unwrap();
    let calls = std::fs::read_to_string(&log).unwrap();
    let walks = calls.lines().filter(|l| l.contains(" log ")).count();
    // One history walk and one object read for 40 specs, plus one alias walk.
    assert_eq!(walks, 2, "{calls}");
    // The history walk is limited by the objects directory, not by one
    // pathspec per spec (git checks every pathspec against every commit).
    assert!(
        calls
            .lines()
            .any(|l| l.contains(" log ") && l.ends_with("--name-only -- objects")),
        "{calls}"
    );
    assert_eq!(calls.lines().count(), 3, "{calls}");
}

/// TASK-1662: a store-history walk that outlives its deadline is an error
/// (the ship path's HistoryUnreadable refusal), never an unbounded wait.
// trace:TASK-1951 trace:TASK-1662 | ai:claude
#[test]
fn task_1951_store_history_walk_times_out_as_unreadable() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH")
        .display()
        .to_string();
    let store = bug_2179_store();
    let shim = tempfile::tempdir().unwrap();
    crate::test_exec::write_executable(
        &shim.path().join("git"),
        format!(
            "#!/bin/sh\ncase \"$*\" in *' log '*) exec /bin/sleep 30;; esac\nexec '{real}' \"$@\"\n"
        ),
    );
    let path = format!(
        "{}:{}",
        shim.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let _guard = crate::test_env::EnvVarsGuard::apply(&[
        ("PATH", Some(&path)),
        ("AIDA_STORE_HISTORY_TIMEOUT_SECS", Some("1")),
    ]);
    let started = std::time::Instant::now();
    let err = store_history_alias_map(store.path(), &["TASK-1".to_string()]).unwrap_err();
    assert!(err.to_string().contains("timed out"), "{err}");
    // TASK-2141: the refusal names the override. trace:TASK-1951 trace:TASK-2141 | ai:claude
    assert!(
        err.to_string().contains("AIDA_STORE_HISTORY_TIMEOUT_SECS"),
        "{err}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
}

// ── TASK-2238: post-merge review notes on the store-history walks ──────────

/// `git` in `root`, returning trimmed stdout.
// trace:TASK-2238 | ai:claude
fn git_stdout(root: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// TASK-2229: the other side of a rename at a non-canonical object path
/// would hold versions never judged, so the alias walk refuses instead of
/// silently dropping it.
// trace:TASK-2238 | ai:claude
#[test]
fn task_2238_a_rename_to_a_non_canonical_path_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    let canonical = aida_core::object_store::relative_object_path("TASK-1-152").unwrap();
    let path = root.join(&canonical);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut req = plain();
    req.spec_id = Some("TASK-1-152".into());
    req.agreed_id = Some("TASK-176".into());
    std::fs::write(&path, serde_yaml::to_string(&req).unwrap()).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "v1"]);
    assert_eq!(
        store_history_alias_ids(root, "TASK-176").unwrap(),
        vec!["TASK-1-152".to_string()]
    );

    // Renamed into the wrong shard while its agreed_id line changes.
    let stray = root.join("objects/TASK/009/TASK-1-152.yaml");
    std::fs::create_dir_all(stray.parent().unwrap()).unwrap();
    std::fs::remove_file(&path).unwrap();
    req.agreed_id = Some("TASK-177".into());
    std::fs::write(&stray, serde_yaml::to_string(&req).unwrap()).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "renamed out of place"]);
    let walk = git_stdout(
        root,
        &[
            "log",
            "-1",
            "--format=",
            "--name-status",
            "-M",
            "--",
            "objects/",
        ],
    );
    assert!(
        walk.starts_with('R'),
        "the fixture must be a rename: {walk}"
    );

    let err = store_history_alias_ids(root, "TASK-176").unwrap_err();
    assert!(err.contains("non-canonical object path"), "{err}");
    assert!(err.contains("objects/TASK/009/TASK-1-152.yaml"), "{err}");
}

/// TASK-2231: a refs/replace/* object never changes what the store history
/// walks read.
// trace:TASK-2238 | ai:claude
#[test]
fn task_2238_store_history_ignores_replace_objects() {
    let store = bug_2179_store();
    let root = store.path();
    let rel = aida_core::object_store::relative_object_path("TASK-2").unwrap();
    let real = git_stdout(root, &["rev-parse", &format!("HEAD:{rel}")]);
    let mut forged = plain();
    forged.spec_id = Some("TASK-2".into());
    forged.title = "forged".into();
    forged.tags.insert("security".into());
    let file = root.join("forged.yaml");
    std::fs::write(&file, serde_yaml::to_string(&forged).unwrap()).unwrap();
    let fake = git_stdout(root, &["hash-object", "-w", "forged.yaml"]);
    std::fs::remove_file(&file).unwrap();
    git(root, &["replace", &real, &fake]);
    assert!(
        git_stdout(root, &["cat-file", "-p", &real]).contains("forged"),
        "the replacement is live for an ordinary git read"
    );

    let versions = StoreHistoryBatch::read(root, &["TASK-2".to_string()])
        .unwrap()
        .versions("TASK-2")
        .unwrap();
    assert!(!versions.is_empty());
    assert!(
        versions
            .iter()
            .all(|v| v.title == "only" && v.tags.is_empty()),
        "{:?}",
        versions.iter().map(|v| &v.title).collect::<Vec<_>>()
    );
}

/// The Git snapshot's narrower dirt scope grants no ledger bypass: this
/// separately read home ledger still refuses newly excluded or corrupt data.
// trace:BUG-2213 | ai:codex
#[test]
fn bug_2213_mandate_ledger_updates_still_refuse() {
    let ledger_dir = tempfile::tempdir().unwrap();
    let mut c = Case::ok();
    c.ledger = read_exclusions(ledger_dir.path());
    assert!(c.decide().is_ok(), "clean home ledger");

    let mut excluded = plain();
    excluded.tags.insert("security".into());
    note_exclusions(ledger_dir.path(), "TASK-1", [&excluded]);
    c.ledger = read_exclusions(ledger_dir.path());
    assert!(matches!(c.decide(), Err(MandateRefusal::Excluded { .. })));

    let ledger = ledger_dir.path().join(EXCLUSION_LEDGER);
    let mut text = std::fs::read_to_string(&ledger).unwrap();
    text.push_str("{partial coordination-era append\n"); // }
    std::fs::write(&ledger, text).unwrap();
    c.ledger = read_exclusions(ledger_dir.path());
    assert!(matches!(
        c.decide(),
        Err(MandateRefusal::LedgerUnreadable(_))
    ));
}

// ── BUG-2214: a hold a human already cleared is covered — nothing looser ────

// trace:BUG-2214 | ai:claude
#[test]
fn bug_2214_a_human_cleared_hold_lets_the_mandate_cover_the_ship() {
    let mut c = Case::ok();
    c.hold = HoldFacts::ClearedByHuman;
    assert_eq!(c.decide(), Ok(active(false).id));
}

// trace:BUG-2214 | ai:claude
#[test]
fn bug_2214_a_cleared_hold_loosens_no_other_refusal() {
    // Sticky exclusion: a sensitive spec still needs --include-supervised.
    let mut c = Case::ok();
    c.hold = HoldFacts::ClearedByHuman;
    c.specs = vec![tagged("TASK-1", "security")];
    assert!(matches!(c.decide(), Err(MandateRefusal::Excluded { .. })));

    // Always-human: an operator-only spec refuses even with the flag.
    let mut c = Case::ok();
    c.state = MandateState::Active(active(true));
    c.hold = HoldFacts::ClearedByHuman;
    let mut operator_only = spec("TASK-1");
    operator_only.current.as_mut().unwrap().execution_mode =
        Some(aida_core::ExecutionMode::Operator);
    operator_only.history = Ok(vec![operator_only.current.clone().unwrap()]);
    c.specs = vec![operator_only];
    assert!(matches!(
        c.decide(),
        Err(MandateRefusal::AlwaysHuman { .. })
    ));

    // The approval must still cover the live head.
    let mut c = Case::ok();
    c.hold = HoldFacts::ClearedByHuman;
    c.candidates = vec![approval(OLD)];
    c.policy = policy(&c.candidates);
    assert!(matches!(c.decide(), Err(MandateRefusal::StaleApproval(_))));

    // Never held at all (no corroborated clearance) still refuses.
    let mut c = Case::ok();
    c.hold = HoldFacts::None;
    assert_eq!(c.decide(), Err(MandateRefusal::NoHoldMarker));
}

// trace:BUG-2214 | ai:claude
#[test]
fn bug_2214_the_rework_release_path_never_accepts_a_cleared_hold() {
    let mut c = Case::ok();
    c.state = MandateState::Active(active(true));
    c.hold = HoldFacts::ClearedByHuman;
    let release = rework_release_decision(&ShipCheck {
        state: &c.state,
        now: c.now,
        grant: c.grant,
        specs: &c.specs,
        specs_resolved: c.specs_resolved,
        candidates: &c.candidates,
        policy: &c.policy,
        head: c.head,
        override_stale_approval: false,
        ledger: &c.ledger,
        hold: &c.hold,
        carry_over: None,
        changed_paths: Some(&[]),
        protected: crate::mandate_gates::ProtectedPaths::floor_ref(),
        ship_gate: None,
        hold_clear_gate: None,
    });
    assert_eq!(release, Err(MandateRefusal::NoHoldMarker));
}

// ── TASK-2168 / TASK-2314: trace-derived operator-only exclusions ──────────

// trace:TASK-2168.ac200bb9 | ai:agy
// trace:TASK-2168.Tests | ai:agy
// trace:TASK-2314 | ai:agy
#[test]
fn task_2168_completed_operator_only_trace_no_longer_excludes() {
    use aida_core::{ExecutionMode, RequirementStatus};
    let tmp = tempfile::tempdir().unwrap();
    let mut tagged_op = plain();
    tagged_op.tags.insert(OPERATOR_ONLY_TAG.into());
    note_exclusions(tmp.path(), "TASK-963", [&tagged_op]);
    let ledger = read_exclusions(tmp.path());
    assert_eq!(
        ledger.as_ref().unwrap()["TASK-963"].class,
        "operator-only",
        "ledger records the operator-only exclusion"
    );

    // 1. Tag-based operator-only trace (current + history + sticky ledger), Completed and shipped.
    let mut current = tagged_op.clone();
    current.status = RequirementStatus::Completed;
    let mut c = Case::ok();
    c.ledger = ledger.clone();
    c.specs = vec![
        spec("TASK-1"),
        ShipSpec {
            id: "TASK-963".into(),
            aliases: Vec::new(),
            named: false,
            shipped: true,
            current: Some(current.clone()),
            history: Ok(vec![tagged_op.clone(), current.clone()]),
        },
    ];
    assert_eq!(
        c.decide(),
        Ok(active(false).id),
        "trace-derived operator-only tag with Completed + shipped must not exclude"
    );

    // 2. ExecutionMode::Operator trace (current + history), Completed and shipped.
    let mut mode_op = plain();
    mode_op.execution_mode = Some(ExecutionMode::Operator);
    let mut mode_done = mode_op.clone();
    mode_done.status = RequirementStatus::Completed;
    c.specs = vec![
        spec("TASK-1"),
        ShipSpec {
            id: "TASK-964".into(),
            aliases: Vec::new(),
            named: false,
            shipped: true,
            current: Some(mode_done.clone()),
            history: Ok(vec![mode_op, mode_done]),
        },
    ];
    assert_eq!(
        c.decide(),
        Ok(active(false).id),
        "trace-derived ExecutionMode::Operator with Completed + shipped must not exclude"
    );

    // 3. Named directly on the PR's own specs: still AlwaysHuman even when Completed and shipped.
    c.state = MandateState::Active(active(true));
    c.specs = vec![ShipSpec {
        id: "TASK-963".into(),
        aliases: Vec::new(),
        named: true,
        shipped: true,
        current: Some(current.clone()),
        history: Ok(vec![current]),
    }];
    assert_eq!(
        c.decide(),
        Err(MandateRefusal::AlwaysHuman {
            spec: "TASK-963".into(),
            class: "operator-only".into(),
        }),
        "named operator-only spec must still refuse when Completed and shipped"
    );

    // 4. Other exclusions (decide-mode, security, sticky ledger) still exclude when Completed and shipped.
    let mut completed_decide = plain();
    completed_decide.status = RequirementStatus::Completed;
    completed_decide.execution_mode = Some(ExecutionMode::Decide);
    let mut c = Case::ok();
    c.state = MandateState::Active(active(true));
    c.specs = vec![
        spec("TASK-1"),
        ShipSpec {
            id: "TASK-965".into(),
            aliases: Vec::new(),
            named: false,
            shipped: true,
            current: Some(completed_decide.clone()),
            history: Ok(vec![completed_decide]),
        },
    ];
    assert_eq!(
        c.decide(),
        Err(MandateRefusal::AlwaysHuman {
            spec: "TASK-965".into(),
            class: "decide-mode".into(),
        })
    );

    let mut completed_sec = plain();
    completed_sec.status = RequirementStatus::Completed;
    completed_sec.execution_mode = Some(ExecutionMode::Operator);
    completed_sec.tags.insert("security".into());
    let mut c = Case::ok();
    c.specs = vec![
        spec("TASK-1"),
        ShipSpec {
            id: "TASK-966".into(),
            aliases: Vec::new(),
            named: false,
            shipped: true,
            current: Some(completed_sec.clone()),
            history: Ok(vec![completed_sec]),
        },
    ];
    assert!(
        matches!(c.decide(), Err(MandateRefusal::Excluded { ref spec, .. }) if spec == "TASK-966"),
        "a completed operator-only trace that also carries security still excludes without --include-supervised"
    );
}

// trace:TASK-2168.ac200bb9 | ai:agy
// trace:TASK-2168.Tests | ai:agy
#[test]
fn task_2168_approved_or_in_progress_operator_only_trace_still_excludes() {
    use aida_core::{ExecutionMode, RequirementStatus};
    let tmp = tempfile::tempdir().unwrap();
    let mut tagged_op = plain();
    tagged_op.tags.insert(OPERATOR_ONLY_TAG.into());
    note_exclusions(tmp.path(), "TASK-963", [&tagged_op]);
    let ledger = read_exclusions(tmp.path());

    for status in [
        RequirementStatus::Approved,
        RequirementStatus::InProgress,
        RequirementStatus::Draft,
        RequirementStatus::Planned,
        RequirementStatus::Done,
        RequirementStatus::NeedsAttention,
    ] {
        let mut current = tagged_op.clone();
        current.status = status.clone();
        let mut c = Case::ok();
        c.state = MandateState::Active(active(true));
        c.specs = vec![
            spec("TASK-1"),
            ShipSpec {
                id: "TASK-963".into(),
                aliases: Vec::new(),
                named: false,
                shipped: true,
                current: Some(current.clone()),
                history: Ok(vec![current]),
            },
        ];
        assert_eq!(
            c.decide(),
            Err(MandateRefusal::AlwaysHuman {
                spec: "TASK-963".into(),
                class: "operator-only".into(),
            }),
            "trace-derived operator-only spec with status {status:?} must still exclude"
        );

        // Even if the tag was stripped on the current record, the sticky ledger still excludes.
        let mut stripped = plain();
        stripped.status = status.clone();
        c.ledger = ledger.clone();
        c.specs = vec![
            spec("TASK-1"),
            ShipSpec {
                id: "TASK-963".into(),
                aliases: Vec::new(),
                named: false,
                shipped: true,
                current: Some(stripped.clone()),
                history: Ok(vec![stripped]),
            },
        ];
        assert_eq!(
            c.decide(),
            Err(MandateRefusal::AlwaysHuman {
                spec: "TASK-963".into(),
                class: "operator-only".into(),
            }),
            "sticky ledger must still exclude non-terminal status {status:?}"
        );

        let mut mode_op = plain();
        mode_op.status = status.clone();
        mode_op.execution_mode = Some(ExecutionMode::Operator);
        c.ledger = Ok(Default::default());
        c.specs = vec![
            spec("TASK-1"),
            ShipSpec {
                id: "TASK-964".into(),
                aliases: Vec::new(),
                named: false,
                shipped: true,
                current: Some(mode_op.clone()),
                history: Ok(vec![mode_op]),
            },
        ];
        assert_eq!(
            c.decide(),
            Err(MandateRefusal::AlwaysHuman {
                spec: "TASK-964".into(),
                class: "operator-only".into(),
            }),
            "ExecutionMode::Operator trace with status {status:?} must still exclude"
        );
    }
}

// trace:TASK-2168.ac2ecf97 | ai:agy
// trace:TASK-2168.Tests | ai:agy
// trace:TASK-2314 | ai:agy
#[test]
fn task_2168_unreadable_store_for_operator_only_trace_still_excludes() {
    use aida_core::RequirementStatus;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);

    let write_req = |id: &str, req: &aida_core::Requirement| {
        let rel = aida_core::object_store::relative_object_path(id).unwrap();
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_yaml::to_string(req).unwrap()).unwrap();
    };
    let mut r1 = plain();
    r1.spec_id = Some("TASK-1".into());
    write_req("TASK-1", &r1);

    let mut r963 = plain();
    r963.spec_id = Some("TASK-963".into());
    r963.status = RequirementStatus::Completed;
    r963.tags.insert(OPERATOR_ONLY_TAG.into());
    write_req("TASK-963", &r963);
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "commit",
            "-qm",
            "seed completed operator-only spec (TASK-963)",
        ],
    );

    // With a readable store and a merged commit on main, the completed
    // operator-only trace no longer excludes.
    let mut loaded = aida_core::RequirementsStore::new();
    loaded.requirements = vec![r1.clone(), r963.clone()];
    let mut c = Case::ok();
    c.state = MandateState::Active(active(true));
    c.specs = crate::mandate_ship::resolve_exclusion_specs(
        Some(&loaded),
        root,
        &["TASK-1".into()],
        ["TASK-963".into()].into_iter().collect(),
    );
    assert_eq!(c.decide(), Ok(active(true).id));

    // Fail closed: when the live store cannot be read (`store: None`, so
    // `current` is `None`), the exclusion from history/ledger still applies
    // even though the committed object had `status: Completed` and a merged
    // commit on `main`.
    let unresolved_traced = crate::mandate_ship::resolve_exclusion_specs(
        None,
        root,
        &[],
        ["TASK-963".into()].into_iter().collect(),
    );
    assert_eq!(unresolved_traced.len(), 1);
    assert!(unresolved_traced[0].current.is_none());
    c.specs = vec![
        spec("TASK-1"),
        unresolved_traced.into_iter().next().unwrap(),
    ];
    assert_eq!(
        c.decide(),
        Err(MandateRefusal::AlwaysHuman {
            spec: "TASK-963".into(),
            class: "operator-only".into(),
        })
    );

    // And an unreadable git store history also fails closed.
    c.specs = vec![
        spec("TASK-1"),
        ShipSpec {
            id: "TASK-963".into(),
            aliases: Vec::new(),
            named: false,
            shipped: true,
            current: None,
            history: Err("store unreadable".into()),
        },
    ];
    assert!(
        matches!(c.decide(), Err(MandateRefusal::HistoryUnreadable { ref spec, .. }) if spec == "TASK-963")
    );
}

// trace:TASK-2168.ac200bb9 | ai:agy
#[test]
fn task_2168_ledger_decide_mode_always_human_still_refuses_for_completed_trace() {
    use aida_core::{ExecutionMode, RequirementStatus};
    let tmp = tempfile::tempdir().unwrap();
    let mut decide_req = plain();
    decide_req.execution_mode = Some(ExecutionMode::Decide);
    note_exclusions(tmp.path(), "TASK-967", [&decide_req]);
    let ledger = read_exclusions(tmp.path());
    assert_eq!(
        ledger.as_ref().unwrap()["TASK-967"].class,
        "decide-mode",
        "ledger records the decide-mode always_human exclusion"
    );
    assert!(ledger.as_ref().unwrap()["TASK-967"].always_human);

    // Current and history carry no exclusion (or carry an expired operator-only
    // exclusion), so only the sticky ledger's `always_human` decide-mode entry
    // is consulted at mandate.rs:1758.
    let mut completed = plain();
    completed.status = RequirementStatus::Completed;
    completed.tags.insert(OPERATOR_ONLY_TAG.into());
    let mut c = Case::ok();
    c.state = MandateState::Active(active(true));
    c.ledger = ledger;
    c.specs = vec![
        spec("TASK-1"),
        ShipSpec {
            id: "TASK-967".into(),
            aliases: Vec::new(),
            named: false,
            shipped: true,
            current: Some(completed.clone()),
            history: Ok(vec![completed]),
        },
    ];
    assert_eq!(
        c.decide(),
        Err(MandateRefusal::AlwaysHuman {
            spec: "TASK-967".into(),
            class: "decide-mode".into(),
        })
    );
}

// trace:TASK-2168.ac200bb9 | ai:agy
#[test]
fn task_2168_non_always_human_ledger_entry_still_yields_excluded_for_completed_trace() {
    use aida_core::RequirementStatus;
    let tmp = tempfile::tempdir().unwrap();
    let mut security_req = plain();
    security_req.tags.insert("security".into());
    note_exclusions(tmp.path(), "TASK-968", [&security_req]);
    let ledger = read_exclusions(tmp.path());
    assert!(!ledger.as_ref().unwrap()["TASK-968"].always_human);

    // Current and history carry only an expired operator-only tag on a
    // Completed + shipped trace, so only the sticky ledger's non-always_human
    // entry at mandate.rs:1758 supplies `excluded`.
    let mut completed = plain();
    completed.status = RequirementStatus::Completed;
    completed.tags.insert(OPERATOR_ONLY_TAG.into());
    let mut c = Case::ok();
    c.ledger = ledger;
    c.specs = vec![
        spec("TASK-1"),
        ShipSpec {
            id: "TASK-968".into(),
            aliases: Vec::new(),
            named: false,
            shipped: true,
            current: Some(completed.clone()),
            history: Ok(vec![completed]),
        },
    ];
    assert!(
        matches!(
            c.decide(),
            Err(MandateRefusal::Excluded { ref spec, ref why })
                if spec == "TASK-968" && why.contains("recorded")
        ),
        "non-always-human ledger entry on a completed trace must still yield Excluded"
    );
}

// trace:TASK-2168.ac200bb9 | ai:agy
// trace:TASK-2168.Tests | ai:agy
// trace:TASK-2314 | ai:agy
#[test]
fn task_2314_rejected_operator_only_trace_still_excludes() {
    use aida_core::{ExecutionMode, RequirementStatus};
    let tmp = tempfile::tempdir().unwrap();
    let mut tagged_op = plain();
    tagged_op.tags.insert(OPERATOR_ONLY_TAG.into());
    note_exclusions(tmp.path(), "TASK-963", [&tagged_op]);
    let ledger = read_exclusions(tmp.path());

    for shipped in [false, true] {
        let mut rejected_tag = tagged_op.clone();
        rejected_tag.status = RequirementStatus::Rejected;
        let mut c = Case::ok();
        c.state = MandateState::Active(active(true));
        c.ledger = ledger.clone();
        c.specs = vec![
            spec("TASK-1"),
            ShipSpec {
                id: "TASK-963".into(),
                aliases: Vec::new(),
                named: false,
                shipped,
                current: Some(rejected_tag.clone()),
                history: Ok(vec![tagged_op.clone(), rejected_tag]),
            },
        ];
        assert_eq!(
            c.decide(),
            Err(MandateRefusal::AlwaysHuman {
                spec: "TASK-963".into(),
                class: "operator-only".into(),
            }),
            "Rejected operator-only tag trace (shipped={shipped}) must still exclude"
        );

        let mut rejected_mode = plain();
        rejected_mode.status = RequirementStatus::Rejected;
        rejected_mode.execution_mode = Some(ExecutionMode::Operator);
        c.ledger = Ok(Default::default());
        c.specs = vec![
            spec("TASK-1"),
            ShipSpec {
                id: "TASK-964".into(),
                aliases: Vec::new(),
                named: false,
                shipped,
                current: Some(rejected_mode.clone()),
                history: Ok(vec![rejected_mode]),
            },
        ];
        assert_eq!(
            c.decide(),
            Err(MandateRefusal::AlwaysHuman {
                spec: "TASK-964".into(),
                class: "operator-only".into(),
            }),
            "Rejected ExecutionMode::Operator trace (shipped={shipped}) must still exclude"
        );
    }
}

// trace:TASK-2168.ac200bb9 | ai:agy
// trace:TASK-2168.Tests | ai:agy
// trace:TASK-2314 | ai:agy
#[test]
fn task_2314_superseded_operator_only_trace_still_excludes() {
    use aida_core::{ExecutionMode, RequirementStatus};
    let tmp = tempfile::tempdir().unwrap();
    let mut tagged_op = plain();
    tagged_op.tags.insert(OPERATOR_ONLY_TAG.into());
    note_exclusions(tmp.path(), "TASK-963", [&tagged_op]);
    let ledger = read_exclusions(tmp.path());

    for shipped in [false, true] {
        let mut superseded_tag = tagged_op.clone();
        superseded_tag.status = RequirementStatus::Superseded;
        let mut c = Case::ok();
        c.state = MandateState::Active(active(true));
        c.ledger = ledger.clone();
        c.specs = vec![
            spec("TASK-1"),
            ShipSpec {
                id: "TASK-963".into(),
                aliases: Vec::new(),
                named: false,
                shipped,
                current: Some(superseded_tag.clone()),
                history: Ok(vec![tagged_op.clone(), superseded_tag]),
            },
        ];
        assert_eq!(
            c.decide(),
            Err(MandateRefusal::AlwaysHuman {
                spec: "TASK-963".into(),
                class: "operator-only".into(),
            }),
            "Superseded operator-only tag trace (shipped={shipped}) must still exclude"
        );

        let mut superseded_mode = plain();
        superseded_mode.status = RequirementStatus::Superseded;
        superseded_mode.execution_mode = Some(ExecutionMode::Operator);
        c.ledger = Ok(Default::default());
        c.specs = vec![
            spec("TASK-1"),
            ShipSpec {
                id: "TASK-964".into(),
                aliases: Vec::new(),
                named: false,
                shipped,
                current: Some(superseded_mode.clone()),
                history: Ok(vec![superseded_mode]),
            },
        ];
        assert_eq!(
            c.decide(),
            Err(MandateRefusal::AlwaysHuman {
                spec: "TASK-964".into(),
                class: "operator-only".into(),
            }),
            "Superseded ExecutionMode::Operator trace (shipped={shipped}) must still exclude"
        );
    }
}

// trace:TASK-2168.ac200bb9 | ai:agy
// trace:TASK-2168.Tests | ai:agy
// trace:TASK-2314 | ai:agy
#[test]
fn task_2314_completed_without_shipped_commit_still_excludes() {
    use aida_core::{ExecutionMode, RequirementStatus};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);

    let write_req = |id: &str, req: &aida_core::Requirement| {
        let rel = aida_core::object_store::relative_object_path(id).unwrap();
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_yaml::to_string(req).unwrap()).unwrap();
    };
    let mut r1 = plain();
    r1.spec_id = Some("TASK-1".into());
    write_req("TASK-1", &r1);

    let mut r963 = plain();
    r963.spec_id = Some("TASK-963".into());
    r963.status = RequirementStatus::Completed;
    r963.tags.insert(OPERATOR_ONLY_TAG.into());
    write_req("TASK-963", &r963);

    let mut r964 = plain();
    r964.spec_id = Some("TASK-964".into());
    r964.status = RequirementStatus::Completed;
    r964.execution_mode = Some(ExecutionMode::Operator);
    write_req("TASK-964", &r964);

    // Even when a tracked file on `main` carries `trace:TASK-963` and a store
    // bookkeeping commit (`update TASK-963`) exists, there is no merged code
    // commit referencing TASK-963 or TASK-964 on `main`.
    std::fs::write(
        root.join("lib.rs"),
        "// trace:TASK-963 | ai:claude\npub fn gated() {}\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "update TASK-963"]);

    let mut loaded = aida_core::RequirementsStore::new();
    loaded.requirements = vec![r1, r963, r964];

    for traced_id in ["TASK-963", "TASK-964"] {
        let specs = crate::mandate_ship::resolve_exclusion_specs(
            Some(&loaded),
            root,
            &["TASK-1".into()],
            [traced_id.into()].into_iter().collect(),
        );
        let traced_spec = specs.iter().find(|s| s.id == traced_id).unwrap();
        assert!(
            !traced_spec.shipped,
            "{traced_id} has no shipped commit on main and must resolve with shipped=false"
        );
        let mut c = Case::ok();
        c.state = MandateState::Active(active(true));
        c.specs = specs;
        assert_eq!(
            c.decide(),
            Err(MandateRefusal::AlwaysHuman {
                spec: traced_id.into(),
                class: "operator-only".into(),
            }),
            "Completed {traced_id} without a shipped commit on main must still exclude"
        );
    }
}

// trace:TASK-2168.ac200bb9 | ai:agy
// trace:TASK-2168.Tests | ai:agy
// trace:TASK-2314 | ai:agy
#[test]
fn task_2314_completed_and_shipped_on_main_expires() {
    use aida_core::{ExecutionMode, RequirementStatus};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);

    let write_req = |id: &str, req: &aida_core::Requirement| {
        let rel = aida_core::object_store::relative_object_path(id).unwrap();
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_yaml::to_string(req).unwrap()).unwrap();
    };
    let mut r1 = plain();
    r1.spec_id = Some("TASK-1".into());
    write_req("TASK-1", &r1);

    let mut r963 = plain();
    r963.spec_id = Some("TASK-963".into());
    r963.status = RequirementStatus::Completed;
    r963.tags.insert(OPERATOR_ONLY_TAG.into());
    write_req("TASK-963", &r963);

    let mut r964 = plain();
    r964.spec_id = Some("TASK-964".into());
    r964.status = RequirementStatus::Completed;
    r964.execution_mode = Some(ExecutionMode::Operator);
    write_req("TASK-964", &r964);

    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "commit",
            "-qm",
            "feat: land operator changes (TASK-963, TASK-964)",
        ],
    );

    let mut loaded = aida_core::RequirementsStore::new();
    loaded.requirements = vec![r1, r963, r964];

    let specs = crate::mandate_ship::resolve_exclusion_specs(
        Some(&loaded),
        root,
        &["TASK-1".into()],
        ["TASK-963".into(), "TASK-964".into()].into_iter().collect(),
    );
    for id in ["TASK-963", "TASK-964"] {
        let s = specs.iter().find(|s| s.id == id).unwrap();
        assert!(s.shipped, "{id} has a merged commit on main: shipped=true");
    }
    let mut c = Case::ok();
    c.specs = specs;
    assert_eq!(
        c.decide(),
        Ok(active(false).id),
        "Completed and shipped operator-only traces expire"
    );
}

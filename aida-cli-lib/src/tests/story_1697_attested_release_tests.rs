//! STORY-1697: an attested approval at the PR's current head releases its
//! rework hold in place — through the same `auto_release_with` path
//! `aida review record` runs, with the real BUG-1918 approval gate and the
//! real STORY-1698 mandate decision. Every input is injected: no TTY, no
//! forge, no environment reads or writes.
// trace:STORY-1697 | ai:claude

use super::*;
use crate::mandate::{HoldFacts, Mandate, MandateState, ShipCheck, ShipSpec};
use crate::merge_hold::HoldReasonKind;
use crate::pr_ship::{AuthorSession, ShipApprovalPolicy};
use crate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};
use chrono::Utc;
use std::cell::RefCell;

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OLD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const PR: u64 = 37;
/// The independent reviewer session that recorded the approval.
const RECORDER: &str = "session:independent-reviewer";
/// The advisor session running the release (the mandate's shipper).
const RELEASER: &str = "session:advisor-releaser";
const MANDATE_ID: &str = "6f1c1d6e-0000-4000-8000-000000000097";

fn mandate(include_supervised: bool) -> Mandate {
    Mandate {
        id: MANDATE_ID.into(),
        principal: "joe".into(),
        seat: crate::mandate::MANDATE_SEAT.into(),
        scope: crate::mandate::SCOPE_SHIP_HELD.into(),
        include_supervised,
        reason: Some("fire".into()),
        issued_at: Utc::now() - chrono::Duration::hours(1),
        expires_at: Utc::now() + chrono::Duration::hours(7),
        revoked_at: None,
        revoked_by: None,
        scopes: Default::default(),
        kind: None,
    }
}

/// The approval as the merge gates read it: BUG-1918-attested at `sha`.
fn candidate(sha: &str) -> RecordedVerdict {
    RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(sha.into()),
        recorded_at: Some("2026-10-14T01:00:02Z".into()),
        attestation: Some(RecorderAttestation {
            sha: sha.into(),
            human_at_tty: true,
            receipt_id: Some("rcpt".into()),
            identity: vec![RECORDER.into()],
            ..Default::default()
        }),
        ..Default::default()
    }
}

struct Case {
    hold: mh::MergeHoldRecord,
    body: Value,
    req: aida_core::Requirement,
    grant: sa::SeatGrant,
    candidates: Vec<RecordedVerdict>,
    policy: ShipApprovalPolicy,
    state: MandateState,
    /// `(seat, principal)` the mandate decision sees for the releasing session.
    seat: (&'static str, &'static str),
    /// Stop the mandate after its first consultation (eligibility), before
    /// the release-time recheck.
    revoke_mid_release: bool,
    /// Fail the label removal, the release's last step (TASK-1915).
    fail_labels: bool,
}

impl Case {
    /// The happy path: a typed rework hold on an ordinary spec, and an
    /// independent reviewer's attested approval recorded at the current head;
    /// the advisor session ([`RELEASER`]) runs the release.
    fn ok() -> Self {
        let grant = sa::SeatGrant {
            id: uuid::Uuid::new_v4().to_string(),
            principal: "joe".into(),
            subject: "reviewer".into(),
            session_id: "reviewer-session".into(),
            seat: "reviewer".into(),
            tty_issued_at: Utc::now(),
            delegable_seats: vec![],
            parent_grant_id: None,
            issued_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
            revoked_at: None,
        };
        let mut hold = mh::typed_hold(PR, HoldReasonKind::Rework, "rework", Some(OLD.into()));
        hold.verdict_ref = Some(mh::VerdictRef::new(
            "TASK-37",
            Some(PR),
            Some(OLD.into()),
            Some("old reviewer".into()),
        ));
        hold.spec = Some("TASK-37".into());
        let mut req = aida_core::Requirement::new("Improve a list".into(), "fixture".into());
        req.req_type = aida_core::RequirementType::Task;
        req.execution_mode = Some(aida_core::ExecutionMode::Drive);
        req.spec_id = Some("TASK-37".into());
        let body = serde_json::json!({"verdict":"approved", "reviewed_sha":HEAD,
            "recorded_at":"2026-10-14T01:00:02Z", "recorded_by":"reviewer",
            "release_grant":{"id":grant.id,"subject":grant.subject,"session_id":grant.session_id,"recorded_by":"reviewer"},
            "rounds":[{"verdict":"request-changes","reviewed_sha":OLD,"recorded_at":"2026-10-14T01:00:01Z","recorded_by":"old reviewer"}]});
        let candidates = vec![candidate(HEAD)];
        let mut policy = ShipApprovalPolicy {
            reviewed_committed_at: Some("2026-10-14T00:00:00Z".parse().unwrap()),
            shipper: vec![RELEASER.into()],
            ..Default::default()
        };
        for v in &candidates {
            let a = v.attestation.as_ref().unwrap();
            let key = a.authority_key(v.recorded_at.as_deref()).unwrap();
            policy.verified_authority.insert(key, a.identity.clone());
        }
        Self {
            hold,
            body,
            req,
            grant,
            candidates,
            policy,
            state: MandateState::None,
            seat: ("advisor", "joe"),
            revoke_mid_release: false,
            fail_labels: false,
        }
    }

    fn sensitive(mut self) -> Self {
        self.req.title = "Rotate the credential store".into();
        self
    }

    /// Run the shared production path against a temp hold root. Returns the
    /// outcome, the audit comments posted, and whether the hold survived.
    fn run(&self) -> (Result<Outcome>, Vec<String>, bool) {
        let root = tempfile::tempdir().unwrap();
        mh::write_typed_hold(root.path(), &self.hold).unwrap();
        let verdict_path = rv::verdict_path(root.path(), "TASK-37");
        std::fs::create_dir_all(verdict_path.parent().unwrap()).unwrap();
        std::fs::write(&verdict_path, serde_json::to_vec(&self.body).unwrap()).unwrap();
        let mut store = aida_core::RequirementsStore::default();
        store.requirements.push(self.req.clone());
        let comments = RefCell::new(Vec::new());
        let specs = vec![ShipSpec {
            id: "TASK-37".into(),
            aliases: Vec::new(),
            named: true,
            current: Some(self.req.clone()),
            history: Ok(vec![self.req.clone()]),
        }];
        let ledger = Ok(Default::default());
        let consulted = std::cell::Cell::new(0usize);
        let result = auto_release_with(
            root.path(),
            &self.hold,
            AutoReleaseIo {
                live_head: &|| {
                    Ok(Some(mh::PinnedChange {
                        merged: false,
                        state: mh::ChangeState::Open,
                        head_sha: Some(HEAD.into()),
                        hold_label: true,
                        recorded_label: true,
                        cleared_label: false,
                    }))
                },
                read_hold: &|| mh::read_hold_record(root.path(), PR),
                covered_specs: &|| Ok((vec!["TASK-37".into()], store.clone())),
                read_body: &|key| {
                    Ok(serde_json::from_slice(&std::fs::read(rv::verdict_path(
                        root.path(),
                        key,
                    ))?)?)
                },
                read_history: &|_| Ok(RetainedHistory::default()),
                read_corpus: &|ids| {
                    mh::corpus_gate_bodies(&[root.path()], PR, ids).map_err(anyhow::Error::msg)
                },
                grant_check: &|_, _| sa::GrantCheck::Valid(self.grant.clone()),
                approval_authority: &|head| {
                    approval_attested(&self.candidates, &self.policy, head, PR)
                },
                mandate: &|| {
                    let hold = mh::read_hold_record(root.path(), PR);
                    let state = match &self.state {
                        MandateState::Active(m)
                            if self.revoke_mid_release && consulted.get() > 0 =>
                        {
                            let mut m = m.clone();
                            m.revoked_at = Some(Utc::now());
                            MandateState::Stopped(m)
                        }
                        state => state.clone(),
                    };
                    consulted.set(consulted.get() + 1);
                    crate::mandate::rework_release_decision(&ShipCheck {
                        state: &state,
                        now: Utc::now(),
                        grant: Some(self.seat),
                        specs: &specs,
                        specs_resolved: true,
                        candidates: &self.candidates,
                        policy: &self.policy,
                        head: Some(HEAD),
                        override_stale_approval: false,
                        ledger: &ledger,
                        hold: &HoldFacts::of(hold.as_ref(), None),
                        carry_over: None,
                        changed_paths: Some(&[]),
                        protected: crate::mandate_gates::ProtectedPaths::floor_ref(),
                        ship_gate: None,
                        hold_clear_gate: None,
                    })
                    .map_err(|refusal| anyhow::anyhow!("{}", refusal.message(PR)))
                },
                comment: &|body| {
                    comments.borrow_mut().push(body.to_string());
                    Ok(())
                },
                labels: &|| {
                    if self.fail_labels {
                        anyhow::bail!("release labels failed: gh down")
                    }
                    Ok(())
                },
            },
        );
        let held = mh::hold_path(root.path(), PR).exists();
        // TASK-1915 F3: a failed mandate-covered release writes a refused
        // row. trace:TASK-1915 | ai:claude
        if self.fail_labels {
            let refused = crate::mandate_drive::ledger_rows(root.path(), MANDATE_ID)
                .into_iter()
                .filter(|(_, _, r)| {
                    r.action == "release-rework-hold"
                        && r.outcome == crate::mandate_drive::Outcome::Refused
                        && r.why.as_deref().is_some_and(|w| w.contains("gh down"))
                })
                .count();
            assert_eq!(refused, 1);
        }
        if result.is_ok() {
            let audit: Value = serde_json::from_slice(
                &std::fs::read(mh::clearance_path(root.path(), PR)).unwrap(),
            )
            .unwrap();
            assert_eq!(audit["cleared_by"], PRINCIPAL);
            assert_eq!(audit["target_head_sha"], HEAD);
            let under = released_mandate(&result);
            assert_eq!(
                audit.get("released_under_mandate").and_then(Value::as_str),
                under.as_deref()
            );
            // TASK-1915 F3: a mandate release writes its MandateDriveAction
            // row; an ordinary one writes none. trace:TASK-1915 | ai:claude
            let rows = crate::mandate_drive::ledger_rows(root.path(), MANDATE_ID);
            let released: Vec<_> = rows
                .iter()
                .filter(|(_, actor, r)| {
                    actor == "session"
                        && r.action == "release-rework-hold"
                        && r.lane == format!("pr:{PR}")
                        && r.outcome == crate::mandate_drive::Outcome::Done
                })
                .collect();
            assert_eq!(released.len(), usize::from(under.is_some()), "{rows:?}");
        }
        (result, comments.into_inner(), held)
    }

    /// Refused, with `why` in the reason, and every release effect withheld.
    fn refused(&self, why: &str) {
        let (result, comments, held) = self.run();
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains(why), "expected `{why}` in: {error}");
        assert!(held, "the hold must stay armed");
        assert!(comments.is_empty(), "no audit comment on a refusal");
    }
}

fn released_mandate(result: &Result<Outcome>) -> Option<String> {
    match result {
        Ok(Outcome::Released { mandate }) => mandate.clone(),
        _ => None,
    }
}

// ── (a) an attested approval at the head releases the rework hold ───────────

#[test]
fn attested_independent_approval_at_head_releases_the_rework_hold() {
    let (result, comments, held) = Case::ok().run();
    assert_eq!(result.unwrap(), Outcome::Released { mandate: None });
    assert!(!held);
    assert_eq!(comments.len(), 1);
    assert!(
        comments[0].contains("require no human signoff"),
        "{}",
        comments[0]
    );
}

#[test]
fn the_release_line_says_what_was_released() {
    assert_eq!(
        Outcome::Released { mandate: None }.line(PR),
        "PR #37: merge-hold auto-released by recorded verdict"
    );
    let line = Outcome::Released {
        mandate: Some(MANDATE_ID.into()),
    }
    .line(PR);
    assert!(
        line.contains(&format!("released under mandate {MANDATE_ID}")),
        "{line}"
    );
    assert_eq!(Outcome::NoHold.line(PR), "PR #37: no active local hold");
}

#[test]
fn an_approval_of_a_stale_sha_does_not_release() {
    // The recorded approval names an older commit than the PR's head.
    let mut case = Case::ok();
    case.body["reviewed_sha"] = Value::String(OLD.into());
    case.refused("stale");
    // The verdict file is current but the attested approval the merge gate
    // reads covers only an older commit.
    let mut case = Case::ok();
    case.candidates = vec![candidate(OLD)];
    case.refused("PR-37");
}

#[test]
fn an_approval_without_a_recorder_attestation_does_not_release() {
    let mut case = Case::ok();
    case.candidates[0].attestation = None;
    case.refused("carries no verified reviewer identity");
    // An attestation whose authority did not re-validate is unattested too.
    let mut case = Case::ok();
    case.policy.verified_authority.clear();
    case.refused("did not re-validate");
}

#[test]
fn an_implementer_self_approval_does_not_release() {
    // BUG-1918: the approval was recorded by the session bound to authoring
    // the PR.
    let mut case = Case::ok();
    case.policy.author_sessions = vec![AuthorSession {
        label: "lease on TASK-37".into(),
        tokens: vec![RECORDER.into()],
    }];
    case.refused("implementer cannot approve its own work");
    // And an implementer seat's grant never releases, whatever it recorded.
    let mut case = Case::ok();
    case.grant.seat = "implementer".into();
    case.refused("implementer");
}

#[test]
fn a_supervised_origin_hold_never_releases_even_under_a_mandate() {
    for kind in [HoldReasonKind::Supervision, HoldReasonKind::Decision] {
        let mut case = Case::ok();
        case.hold.reason_kind = kind;
        case.state = MandateState::Active(mandate(true));
        case.refused("Rework-only");
    }
}

// ── (b) sensitive specs: held without a mandate, released under one ────────

#[test]
fn a_sensitive_spec_without_a_mandate_stays_held() {
    let case = Case::ok().sensitive();
    case.refused("sensitive spec metadata (credential)");
    case.refused("no mandate is active");
}

#[test]
fn a_mandate_without_include_supervised_does_not_cover_a_sensitive_spec() {
    let mut case = Case::ok().sensitive();
    case.state = MandateState::Active(mandate(false));
    case.refused("--include-supervised");
}

#[test]
fn a_sensitive_rework_hold_releases_under_an_include_supervised_mandate() {
    let mut case = Case::ok().sensitive();
    case.state = MandateState::Active(mandate(true));
    let (result, comments, held) = case.run();
    assert_eq!(
        result.unwrap(),
        Outcome::Released {
            mandate: Some(MANDATE_ID.into())
        }
    );
    assert!(!held);
    assert!(
        comments[0].contains(&format!("released under mandate {MANDATE_ID} by advisor")),
        "{}",
        comments[0]
    );
}

#[test]
fn a_mandate_covers_only_the_advisor_seat_of_its_minting_human() {
    let mut case = Case::ok().sensitive();
    case.state = MandateState::Active(mandate(true));
    case.seat = ("reviewer", "joe");
    case.refused("covers only the advisor seat");
    let mut case = Case::ok().sensitive();
    case.state = MandateState::Active(mandate(true));
    case.seat = ("advisor", "someone-else");
    case.refused("minted by `joe`");
}

#[test]
fn an_expired_or_stopped_mandate_is_inert() {
    let mut expired = mandate(true);
    expired.expires_at = Utc::now() - chrono::Duration::minutes(1);
    let mut stopped = mandate(true);
    stopped.revoked_at = Some(Utc::now());
    for state in [
        MandateState::Expired(expired),
        MandateState::Stopped(stopped),
    ] {
        let mut case = Case::ok().sensitive();
        case.state = state;
        case.refused("sensitive spec metadata");
    }
}

#[test]
fn operator_only_specs_never_release_even_under_a_mandate() {
    let mut case = Case::ok().sensitive();
    case.state = MandateState::Active(mandate(true));
    case.req.tags.insert("operator-only".into());
    case.refused("operator-only");
    let mut case = Case::ok();
    case.state = MandateState::Active(mandate(true));
    case.req.execution_mode = Some(aida_core::ExecutionMode::Operator);
    case.refused("guided/operator");
    let mut case = Case::ok();
    case.state = MandateState::Active(mandate(true));
    case.req.execution_mode = Some(aida_core::ExecutionMode::Decide);
    case.refused("decide-mode");
}

#[test]
fn the_mandate_is_consulted_only_for_a_sensitive_spec() {
    // An ordinary spec releases without reading any mandate at all.
    let case = Case::ok();
    let release = eligible(
        &case.hold,
        &case.body,
        &RetainedHistory::default(),
        HEAD,
        &[Some(&case.req)],
        &sa::GrantCheck::Valid(case.grant.clone()),
        &|| panic!("an ordinary spec must not consult the mandate"),
    )
    .unwrap();
    assert_eq!(release.mandate, None);
}

// ── STORY-1698's two-person rule holds for a mandate release ──────────────
// trace:STORY-1697 | ai:claude

#[test]
fn an_advisor_cannot_release_a_sensitive_hold_on_its_own_approval() {
    // The advisor recorded the approval itself and now runs the release.
    let mut case = Case::ok().sensitive();
    case.state = MandateState::Active(mandate(true));
    case.policy.shipper = vec![RECORDER.into()];
    case.refused("must come from an independent reviewer or a human");
    // Same when only the re-validated authority resolves to the releaser.
    let mut case = Case::ok().sensitive();
    case.state = MandateState::Active(mandate(true));
    let v = &case.candidates[0];
    let key = v
        .attestation
        .as_ref()
        .unwrap()
        .authority_key(v.recorded_at.as_deref())
        .unwrap();
    case.policy
        .verified_authority
        .insert(key, vec![RELEASER.into()]);
    case.refused("must come from an independent reviewer or a human");
    // A releasing session with no checkable identity is refused too.
    let mut case = Case::ok().sensitive();
    case.state = MandateState::Active(mandate(true));
    case.policy.shipper = vec![];
    case.refused("no identity AIDA can check");
}

#[test]
fn an_advisor_releases_a_sensitive_hold_on_an_independent_reviewers_approval() {
    let mut case = Case::ok().sensitive();
    case.state = MandateState::Active(mandate(true));
    assert!(!crate::review_authority::identities_overlap(
        &case.policy.shipper,
        &case.candidates[0].attestation.as_ref().unwrap().identity
    ));
    let (result, _, held) = case.run();
    assert_eq!(
        result.unwrap(),
        Outcome::Released {
            mandate: Some(MANDATE_ID.into())
        }
    );
    assert!(!held);
}

#[test]
fn a_mandate_stopped_mid_release_withholds_the_release() {
    // Covered at eligibility, stopped before the release-time recheck.
    let mut case = Case::ok().sensitive();
    case.state = MandateState::Active(mandate(true));
    case.revoke_mid_release = true;
    case.refused("was stopped");
}

// TASK-1915 F3: a mandate-covered release that fails is ledgered refused.
// trace:TASK-1915 | ai:claude
#[test]
fn a_failed_mandate_covered_release_is_ledgered_as_refused() {
    let mut case = Case::ok().sensitive();
    case.state = MandateState::Active(mandate(true));
    case.fail_labels = true;
    let (result, _, held) = case.run();
    assert!(format!("{:#}", result.unwrap_err()).contains("gh down"));
    assert!(held, "a failed release keeps the hold");
}

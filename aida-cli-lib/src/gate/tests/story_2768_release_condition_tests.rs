//! The read-only release-condition check `aida land status` uses: the
//! auto-release's own first eligibility pass, stopped before any release.
// trace:STORY-2768 | ai:claude
use super::*;
use crate::gate::merge_hold_core::{AbsorbedHold, HoldReasonKind};
use chrono::Utc;

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PR: u64 = 41;

struct Case {
    root: tempfile::TempDir,
    hold: mh::MergeHoldRecord,
    body: Value,
    req: aida_core::Requirement,
    grant: sa::SeatGrant,
}

impl Case {
    /// A rework hold on `PR` answered by a fresh independent approval at the
    /// live head, covering one ordinary spec. The hold is on disk.
    fn new() -> Self {
        let grant = sa::SeatGrant {
            id: uuid::Uuid::new_v4().to_string(),
            principal: "operator".into(),
            subject: "reviewer".into(),
            session_id: "review-session".into(),
            seat: "reviewer".into(),
            tty_issued_at: Utc::now(),
            delegable_seats: vec![],
            parent_grant_id: None,
            issued_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
            revoked_at: None,
            binding: Vec::new(),
            origin: None,
            launch_capability: None,
        };
        let mut hold = mh::typed_hold(PR, HoldReasonKind::Rework, "rework", Some(HEAD.into()));
        hold.verdict_ref = Some(mh::VerdictRef::new(
            "TASK-41",
            Some(PR),
            Some(HEAD.into()),
            Some("old reviewer".into()),
        ));
        hold.spec = Some("TASK-41".into());
        let mut req = aida_core::Requirement::new("Improve a list".into(), "fixture".into());
        req.req_type = aida_core::RequirementType::Task;
        req.execution_mode = Some(aida_core::ExecutionMode::Drive);
        req.spec_id = Some("TASK-41".into());
        let body = serde_json::json!({"verdict":"approved", "reviewed_sha":HEAD,
            "recorded_at":"2026-11-13T01:00:02Z", "recorded_by":"fresh reviewer",
            "release_grant":{"id":grant.id,"subject":grant.subject,"session_id":grant.session_id,"recorded_by":"fresh reviewer"},
            "rounds":[{"verdict":"request-changes","reviewed_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","recorded_at":"2026-11-13T01:00:01Z","recorded_by":"old reviewer"}]});
        let root = tempfile::tempdir().unwrap();
        mh::place_hold(root.path(), &hold).unwrap();
        Self {
            root,
            hold,
            body,
            req,
            grant,
        }
    }

    fn sensitive(mut self) -> Self {
        self.req.tags.insert("keystone".into());
        self
    }

    /// Run the auto-release steps with `mandate` as the mandate lookup. Any
    /// release effect (audit comment, label drop) fails the test when
    /// `judge_only`.
    fn run(&self, judge_only: bool, mandate: &dyn Fn() -> Result<String>) -> Result<Judged> {
        let mut store = aida_core::RequirementsStore::default();
        store.requirements.push(self.req.clone());
        let effect = |what: &str| -> Result<()> {
            assert!(!judge_only, "a judge-only run reached the {what}");
            Ok(())
        };
        auto_release_steps(
            self.root.path(),
            &self.hold,
            AutoReleaseIo {
                live_head: &|| {
                    Ok(Some(mh::PinnedChange {
                        state: mh::ChangeState::Open,
                        head_sha: Some(HEAD.into()),
                        merged: false,
                        hold_label: true,
                        recorded_label: true,
                        cleared_label: false,
                    }))
                },
                read_hold: &|| mh::read_hold_record(self.root.path(), PR),
                covered_specs: &|| Ok((vec!["TASK-41".into()], store.clone())),
                read_body: &|_| Ok(self.body.clone()),
                read_history: &|_| Ok(RetainedHistory::default()),
                read_corpus: &|_| Ok(vec![("TASK-41".into(), self.body.to_string())]),
                grant_check: &|_, _| sa::GrantCheck::Valid(self.grant.clone()),
                approval_authority: &|_| Ok(()),
                coverage: &|_| Ok(None),
                mandate,
                refusal_release: &|_, _| held_check(),
                comment: &|_| effect("audit comment"),
                labels: &|| effect("label drop"),
            },
            judge_only,
        )
    }

    fn armed_and_untouched(&self) -> bool {
        mh::read_hold_record(self.root.path(), PR).as_ref() == Some(&self.hold)
            && mh::recorded_clearance(self.root.path(), PR).is_none()
            && crate::gate::ledger::hold::state(self.root.path(), PR)
                .authorized
                .is_none()
    }

    fn condition(&self, mandate: &dyn Fn() -> Result<String>) -> ReleaseCondition {
        condition_of(self.run(true, mandate).map(Some), false)
    }
}

fn no_mandate() -> Result<String> {
    bail!("no mandate is active")
}

fn mandate_named() -> Result<String> {
    Ok("m-1".into())
}

#[test]
fn a_met_condition_is_reported_without_releasing_anything() {
    let case = Case::new();
    assert_eq!(case.condition(&no_mandate), ReleaseCondition::Met);
    assert!(case.armed_and_untouched());
}

#[test]
fn a_sensitive_spec_is_pending_cover_and_the_release_decides_again() {
    let case = Case::new().sensitive();
    // Status reads only that a covering mandate is active.
    assert_eq!(
        case.condition(&mandate_named),
        ReleaseCondition::PendingCover("m-1".into())
    );
    assert!(case.armed_and_untouched());

    // At release the mandate's own cover refuses (as `mandate_ship_cover`
    // does for an excluded spec or a missing grant): the release fails and
    // the hold stays armed, whatever status showed.
    let cover_refuses = || -> Result<String> { bail!("TASK-41 is in an excluded class") };
    let error = match case.run(false, &cover_refuses) {
        Err(error) => format!("{error:#}"),
        Ok(_) => panic!("the release went through on a refused cover"),
    };
    assert!(error.contains("TASK-41 is in an excluded class"), "{error}");
    assert!(case.armed_and_untouched());
}

#[test]
fn a_sensitive_spec_without_a_covering_mandate_is_unmet() {
    let case = Case::new().sensitive();
    match case.condition(&no_mandate) {
        ReleaseCondition::Unmet(why) => {
            assert!(
                why.contains("keystone") && why.contains("no mandate is active"),
                "{why}"
            )
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_always_human_spec_is_unmet_even_under_a_mandate() {
    let mut case = Case::new();
    case.req.execution_mode = Some(aida_core::ExecutionMode::Operator);
    match case.condition(&mandate_named) {
        ReleaseCondition::Unmet(why) => {
            assert!(why.contains("always requires a human clear"), "{why}")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn composed_and_legacy_holds_are_unmet() {
    let mut composed = Case::new();
    composed.hold.absorbed.push(AbsorbedHold {
        reason_kind: HoldReasonKind::Recusal,
        detail: "the reviewer recused".into(),
        recused_principals: Vec::new(),
        target_head_sha: Some(HEAD.into()),
        spec: None,
        placed_by: None,
        verdict_ref: None,
    });
    let mut legacy = Case::new();
    legacy.hold.legacy = true;
    for case in [composed, legacy] {
        match case.condition(&mandate_named) {
            ReleaseCondition::Unmet(why) => {
                assert!(why.contains("require a human clear"), "{why}")
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn an_approval_that_does_not_cover_the_live_head_is_unmet() {
    let mut case = Case::new();
    case.body["reviewed_sha"] = Value::String("c".repeat(40));
    match case.condition(&no_mandate) {
        ReleaseCondition::Unmet(why) => assert!(why.contains("stale or unverifiable"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_failed_read_is_unknown_and_a_missing_hold_is_no_hold() {
    assert_eq!(
        condition_of(Err(anyhow::anyhow!("the forge timed out")), true),
        ReleaseCondition::Unknown("the forge timed out".into())
    );
    assert_eq!(
        condition_of(Err(anyhow::anyhow!("the forge timed out")), false),
        ReleaseCondition::Unmet("the forge timed out".into())
    );
    assert_eq!(condition_of(Ok(None), false), ReleaseCondition::NoHold);
}

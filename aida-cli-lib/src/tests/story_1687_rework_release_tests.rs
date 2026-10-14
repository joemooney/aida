// trace:STORY-1687 | ai:codex
use super::*;
use crate::merge_hold::{AbsorbedHold, HoldReasonKind};
use chrono::Utc;
use std::cell::Cell;

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
struct Fixture {
    hold: mh::MergeHoldRecord,
    body: Value,
    req: aida_core::Requirement,
    grant: sa::SeatGrant,
}
impl Fixture {
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
        };
        let mut hold = mh::typed_hold(37, HoldReasonKind::Rework, "rework", Some(HEAD.into()));
        hold.verdict_ref = Some(mh::VerdictRef::new(
            "TASK-37",
            Some(37),
            Some(HEAD.into()),
            Some("old reviewer".into()),
        ));
        hold.spec = Some("TASK-37".into());
        let mut req = aida_core::Requirement::new("Improve a list".into(), "fixture".into());
        req.req_type = aida_core::RequirementType::Task;
        req.execution_mode = Some(aida_core::ExecutionMode::Drive);
        let body = serde_json::json!({"verdict":"approved", "reviewed_sha":HEAD,
            "recorded_at":"2026-10-14T01:00:02Z", "recorded_by":"fresh reviewer",
            "release_grant":{"id":grant.id,"subject":grant.subject,"session_id":grant.session_id,"recorded_by":"fresh reviewer"},
            "rounds":[{"verdict":"request-changes","reviewed_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","recorded_at":"2026-10-14T01:00:01Z","recorded_by":"old reviewer"}]});
        Self {
            hold,
            body,
            req,
            grant,
        }
    }
    fn check(&self) -> Result<mh::VerdictRef> {
        eligible(
            &self.hold,
            &self.body,
            HEAD,
            &[Some(&self.req)],
            &sa::GrantCheck::Valid(self.grant.clone()),
        )
    }
    fn refused(&self, why: &str) {
        let error = self.check().unwrap_err().to_string();
        assert!(error.contains(why), "{error}");
    }
}
#[test]
fn current_independent_newer_approval_releases_with_audit() {
    let f = Fixture::new();
    let verdict = f.check().unwrap();
    let root = tempfile::tempdir().unwrap();
    mh::write_typed_hold(root.path(), &f.hold).unwrap();
    let comments = Cell::new(0);
    let labels = Cell::new(0);
    release_with(
        root.path(),
        &f.hold,
        &verdict,
        || Ok(HEAD.into()),
        |body| {
            assert!(
                body.contains(PRINCIPAL) && body.contains(HEAD) && body.contains("fresh reviewer")
            );
            assert!(body.contains(".aida/review-verdicts/TASK-37.json"));
            let audit: Value =
                serde_json::from_slice(&std::fs::read(mh::clearance_path(root.path(), 37))?)
                    .unwrap();
            assert_eq!(audit["cleared_by"], PRINCIPAL);
            assert_eq!(audit["target_head_sha"], HEAD);
            assert_eq!(
                audit["released_by_verdict"]["recorded_by"],
                "fresh reviewer"
            );
            comments.set(comments.get() + 1);
            Ok(())
        },
        || {
            assert!(mh::hold_path(root.path(), 37).exists());
            labels.set(labels.get() + 1);
            Ok(())
        },
    )
    .unwrap();
    assert!(!mh::hold_path(root.path(), 37).exists());
    assert_eq!(comments.get(), 1);
    assert_eq!(labels.get(), 1);
}
#[test]
fn stale_head_does_not_release() {
    let mut f = Fixture::new();
    f.body["reviewed_sha"] = Value::String("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into());
    f.refused("stale");
}
#[test]
fn author_implementer_cannot_release_even_with_operator_recorder_label() {
    let mut f = Fixture::new();
    f.grant.seat = "implementer".into();
    f.body["recorded_by"] = Value::String("aida review record (operator)".into());
    f.body["release_grant"]["recorded_by"] = f.body["recorded_by"].clone();
    f.refused("implementer");
    assert!(!record_seat_allowed(Some("implementer")));
    assert!(record_seat_allowed(Some("reviewer")));
}
#[test]
fn invalid_or_absent_reviewer_grant_cannot_release() {
    let f = Fixture::new();
    for authority in [
        sa::GrantCheck::Absent,
        sa::GrantCheck::Rejected {
            id: f.grant.id.clone(),
            seat: Some("reviewer".into()),
            reason: sa::GrantRejection::Unreadable,
        },
    ] {
        assert!(
            eligible(&f.hold, &f.body, HEAD, &[Some(&f.req)], &authority)
                .unwrap_err()
                .to_string()
                .contains("grant")
        );
    }
}
#[test]
fn changed_reviewer_identity_or_unattested_artifact_stays_held() {
    for field in ["id", "subject", "session_id", "recorded_by"] {
        let mut f = Fixture::new();
        f.body["release_grant"][field] = Value::String("author".into());
        f.refused("identity");
    }
    let mut f = Fixture::new();
    f.body.as_object_mut().unwrap().remove("release_grant");
    f.refused("provenance");
}
#[test]
fn approval_must_be_strictly_newer_than_every_arming_refusal() {
    for at in [
        "2026-10-14T01:00:02Z",
        "2026-10-14T01:00:03Z",
        "2026-10-13T18:00:03-07:00",
    ] {
        let mut f = Fixture::new();
        f.body["rounds"][0]["recorded_at"] = Value::String(at.into());
        f.refused("strictly newer");
    }
    let mut f = Fixture::new();
    f.body["rounds"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
        "verdict":"rejected", "recorded_at":"2026-10-14T01:00:03Z"}));
    f.refused("strictly newer");
}
#[test]
fn missing_or_ambiguous_arming_provenance_stays_held() {
    for body in [
        serde_json::json!([]),
        serde_json::json!([{"verdict":"request-changes"}]),
        serde_json::json!([{"verdict":"unknown"}]),
        serde_json::json!([{"verdict":"approved"}]),
    ] {
        let mut f = Fixture::new();
        f.body["rounds"] = body;
        assert!(f.check().is_err());
    }
}
#[test]
fn every_non_rework_kind_primary_or_absorbed_requires_human() {
    for kind in [
        HoldReasonKind::Supervision,
        HoldReasonKind::Recusal,
        HoldReasonKind::Decision,
        HoldReasonKind::Unknown,
    ] {
        let mut f = Fixture::new();
        f.hold.reason_kind = kind;
        f.refused("Rework-only");
        f.hold.reason_kind = HoldReasonKind::Rework;
        f.hold.absorbed.push(AbsorbedHold {
            reason_kind: kind,
            detail: "composed".into(),
            recused_principals: vec![],
            target_head_sha: None,
            spec: None,
            placed_by: None,
            verdict_ref: None,
        });
        f.refused("Rework-only");
    }
    let mut f = Fixture::new();
    f.hold.legacy = true;
    f.refused("Rework-only");
}
#[test]
fn each_sensitive_metadata_trigger_requires_human() {
    for term in [
        "sandbox",
        "contained",
        "confinement",
        "security",
        "authority",
        "grant",
        "identity",
        "credential",
        "secret",
    ] {
        let mut f = Fixture::new();
        f.req.title = format!("Improve {term}");
        f.refused("sensitive");
        f.req.title = "Ordinary task".into();
        f.req.tags.insert(format!("area:{}", term.to_uppercase()));
        f.refused("sensitive");
    }
    for mode in [
        aida_core::ExecutionMode::Guided,
        aida_core::ExecutionMode::Operator,
    ] {
        let mut f = Fixture::new();
        f.req.execution_mode = Some(mode);
        f.refused("guided/operator");
    }
    let mut f = Fixture::new();
    f.req.req_type = aida_core::RequirementType::Epic;
    f.refused("keystone");
    let mut f = Fixture::new();
    f.req.tags.insert("keystone".into());
    f.refused("keystone");
}
#[test]
fn missing_covered_specs_and_one_sensitive_member_stay_held() {
    let f = Fixture::new();
    let grant = sa::GrantCheck::Valid(f.grant.clone());
    assert!(eligible(&f.hold, &f.body, HEAD, &[], &grant).is_err());
    assert!(eligible(&f.hold, &f.body, HEAD, &[None], &grant).is_err());
    let mut other = f.req.clone();
    other.title = "Credential logging".into();
    assert!(eligible(
        &f.hold,
        &f.body,
        HEAD,
        &[Some(&f.req), Some(&other)],
        &grant
    )
    .is_err());
}
#[test]
fn moved_head_before_release_preserves_marker_and_labels() {
    let f = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    mh::write_typed_hold(root.path(), &f.hold).unwrap();
    let count = Cell::new(0);
    let error = release_with(
        root.path(),
        &f.hold,
        &f.check().unwrap(),
        || {
            count.set(count.get() + 1);
            Ok(if count.get() == 1 {
                HEAD.into()
            } else {
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()
            })
        },
        |_| Ok(()),
        || panic!("must not drop labels"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("head moved"));
    assert!(mh::hold_path(root.path(), 37).exists());
}
#[test]
fn failed_comment_or_label_keeps_hold_and_label_retry_does_not_repeat_comment() {
    let f = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    mh::write_typed_hold(root.path(), &f.hold).unwrap();
    let verdict = f.check().unwrap();
    assert!(release_with(
        root.path(),
        &f.hold,
        &verdict,
        || Ok(HEAD.into()),
        |_| bail!("comment failed"),
        || panic!()
    )
    .is_err());
    assert!(mh::hold_path(root.path(), 37).exists());
    assert!(release_with(
        root.path(),
        &f.hold,
        &verdict,
        || Ok(HEAD.into()),
        |_| Ok(()),
        || bail!("labels failed")
    )
    .is_err());
    assert!(mh::hold_path(root.path(), 37).exists());
    release_with(
        root.path(),
        &f.hold,
        &verdict,
        || Ok(HEAD.into()),
        |_| panic!("audit was already posted"),
        || Ok(()),
    )
    .unwrap();
    assert!(!mh::hold_path(root.path(), 37).exists());
}
#[test]
fn failed_clearance_write_keeps_hold() {
    let f = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    mh::write_typed_hold(root.path(), &f.hold).unwrap();
    std::fs::create_dir_all(mh::clearance_path(root.path(), 37)).unwrap();
    assert!(release_with(
        root.path(),
        &f.hold,
        &f.check().unwrap(),
        || Ok(HEAD.into()),
        |_| panic!(),
        || panic!()
    )
    .is_err());
    assert!(mh::hold_path(root.path(), 37).exists());
}

#[test]
fn expired_revoked_subject_roster_and_parent_grants_stay_held() {
    let f = Fixture::new();
    for reason in [
        sa::GrantRejection::Expired { at: Utc::now() },
        sa::GrantRejection::Revoked { at: Utc::now() },
        sa::GrantRejection::SubjectMismatch {
            grant_subject: "author".into(),
            caller: "reviewer".into(),
        },
        sa::GrantRejection::RosterDenied {
            principal: "operator".into(),
            seat: "reviewer".into(),
        },
        sa::GrantRejection::Parent(Box::new(sa::GrantRejection::Unreadable)),
        sa::GrantRejection::ParentNotDelegating {
            parent_seat: "implementer".into(),
        },
    ] {
        let authority = sa::GrantCheck::Rejected {
            id: f.grant.id.clone(),
            seat: Some("reviewer".into()),
            reason,
        };
        let error = eligible(&f.hold, &f.body, HEAD, &[Some(&f.req)], &authority).unwrap_err();
        assert!(error.to_string().starts_with("reviewer grant rejected:"));
    }
}

#[test]
fn new_artifact_writer_cannot_inherit_release_grant() {
    let f = Fixture::new();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("verdict.json");
    std::fs::write(&path, serde_json::to_vec(&f.body).unwrap()).unwrap();
    let obj = rv::build_verdict_object(
        root.path(),
        &path,
        Some("approved"),
        Some(HEAD),
        None,
        Some("new direct writer"),
        &[],
        "different reviewer",
    )
    .unwrap();
    assert!(!obj.contains_key("release_grant"));
    assert!(obj.get("rounds").is_some());
}

#[test]
fn auto_release_cli_supports_one_or_all_and_has_no_override() {
    use clap::Parser;
    assert!(crate::cli::Cli::try_parse_from(["aida", "merge-hold", "auto-release"]).is_ok());
    assert!(crate::cli::Cli::try_parse_from(["aida", "merge-hold", "auto-release", "37"]).is_ok());
    for flag in ["--force", "--override", "--yes"] {
        assert!(crate::cli::Cli::try_parse_from([
            "aida",
            "merge-hold",
            "auto-release",
            "37",
            flag
        ])
        .is_err());
    }
}

#[test]
fn happy_fixture_also_passes_existing_corpus_gate() {
    let f = Fixture::new();
    let bodies = vec![("TASK-37".into(), serde_json::to_string(&f.body).unwrap())];
    assert!(matches!(
        mh::corpus_merge_gate(37, &bodies, Some(HEAD)),
        mh::CorpusMergeGate::Clear
    ));
}

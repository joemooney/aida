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

// Drive auto_release's actual eligibility and release-time validation closure,
// rather than supplying a test validation callback to release_with.
// trace:STORY-1687 | ai:codex
#[derive(Clone, Copy, Default)]
struct ReleaseChange {
    move_head: bool,
    stricter_hold: bool,
    changed_rework: bool,
    rejected_grant: bool,
    rewritten_approval: bool,
    changed_recorder: bool,
    corpus_key: Option<&'static str>,
    fresh_corpus_id: bool,
    after_audit: bool,
    failed_read: Option<&'static str>,
    forge_state: Option<mh::ChangeState>,
    missing_change: bool,
    missing_head: bool,
    absorbed_kind: Option<HoldReasonKind>,
    before_eligibility: bool,
    approval_field: Option<&'static str>,
    grant_field: Option<&'static str>,
    spec_field: Option<&'static str>,
    hold_field: Option<&'static str>,
}

fn auto_release_fixture(change: ReleaseChange) {
    let mut f = Fixture::new();
    f.req.spec_id = Some("TASK-37".into());
    if change.spec_field == Some("agreed_id") {
        f.req.spec_id = None;
        f.req.agreed_id = Some("TASK-37".into());
    }
    let root = tempfile::tempdir().unwrap();
    if change
        .hold_field
        .is_some_and(|field| field.starts_with("/absorbed/"))
    {
        f.hold.absorbed.push(absorbed(HoldReasonKind::Rework));
    }
    if change
        .hold_field
        .is_some_and(|field| field.starts_with("/recused_principals/"))
    {
        f.hold
            .recused_principals
            .push(mh::PrincipalIdentity::human("old"));
    }
    if change
        .hold_field
        .is_some_and(|field| field.starts_with("/routed_to/"))
    {
        f.hold.routed_to.push(mh::PrincipalIdentity::human("old"));
    }
    if change.before_eligibility {
        if let Some(kind) = change.absorbed_kind {
            f.hold.absorbed.push(absorbed(kind));
        }
    }
    mh::write_typed_hold(root.path(), &f.hold).unwrap();
    let verdict_path = rv::verdict_path(root.path(), "TASK-37");
    std::fs::create_dir_all(verdict_path.parent().unwrap()).unwrap();
    std::fs::write(&verdict_path, serde_json::to_vec(&f.body).unwrap()).unwrap();
    let original_marker = std::fs::read(mh::hold_path(root.path(), 37)).unwrap();
    let expected_marker = std::cell::RefCell::new(original_marker);
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(f.req.clone());
    let head_reads = Cell::new(0);
    let hold_reads = Cell::new(0);
    let change_at = if change.before_eligibility {
        1
    } else if change.after_audit {
        3
    } else {
        2
    };
    let corpus_reads = Cell::new(0);
    let body_reads = Cell::new(0);
    let grant_reads = Cell::new(0);
    let spec_reads = Cell::new(0);
    let comments = std::cell::RefCell::new(Vec::new());
    let original_labels = vec![
        "aida:merge-hold".to_string(),
        "aida:merge-hold-recorded".into(),
    ];
    let labels = std::cell::RefCell::new(original_labels.clone());
    let mut expected_hold = f.hold.clone();
    if change.stricter_hold {
        expected_hold.reason_kind = HoldReasonKind::Decision;
        expected_hold.detail = "human decision required".into();
    }
    if change.changed_rework {
        expected_hold.detail = "new Rework authorization required".into();
    }
    if !change.before_eligibility {
        if let Some(kind) = change.absorbed_kind {
            expected_hold.absorbed.push(absorbed(kind));
        }
    }
    if let Some(field) = change.hold_field {
        let mut value = serde_json::to_value(&expected_hold).unwrap();
        if field.starts_with('/') {
            *value.pointer_mut(field).unwrap() = changed_nested_hold_value(field);
        } else {
            value[field] = changed_hold_value(field);
        }
        expected_hold = serde_json::from_value(value).unwrap();
    }
    // A refusal on another covered spec must participate alongside the arming key.
    if let Some(key) = change.corpus_key {
        if key != "PR-37" {
            let mut other = f.req.clone();
            other.spec_id = Some(key.into());
            store.requirements.push(other);
        }
        let path = rv::verdict_path(root.path(), key);
        if !change.after_audit {
            std::fs::write(
                path,
                serde_json::to_vec(&serde_json::json!({
                    "verdict": "request-changes", "reviewed_sha": HEAD,
                    "recorded_at": "2026-10-14T01:00:03Z", "recorded_by": "another reviewer"
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }
    if change.fresh_corpus_id {
        // Present at the current head, but outside the initially covered IDs.
        std::fs::write(
            rv::verdict_path(root.path(), "TASK-40"),
            serde_json::to_vec(&serde_json::json!({
                "verdict": "request-changes", "reviewed_sha": HEAD,
                "recorded_at": "2026-10-14T01:00:03Z", "recorded_by": "another reviewer"
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let ids: Vec<String> = store
        .requirements
        .iter()
        .map(|req| {
            req.spec_id
                .clone()
                .or_else(|| req.agreed_id.clone())
                .unwrap()
        })
        .collect();
    let result = auto_release_with(
        root.path(),
        &f.hold,
        AutoReleaseIo {
            live_head: &|| {
                head_reads.set(head_reads.get() + 1);
                if head_reads.get() >= change_at && change.failed_read == Some("head") {
                    bail!("live head unavailable");
                }
                if head_reads.get() == change_at
                    && (change.stricter_hold
                        || change.changed_rework
                        || (!change.before_eligibility && change.absorbed_kind.is_some())
                        || change.hold_field.is_some())
                {
                    if change.absorbed_kind.is_some() || change.hold_field.is_some() {
                        // Model an atomic marker replacement, without promoting the
                        // primary kind: the entire absorbed record must be compared.
                        aida_core::fs_atomic::write_atomic(
                            &mh::hold_path(root.path(), 37),
                            &serde_json::to_vec_pretty(&expected_hold)?,
                        )?;
                    } else {
                        mh::write_typed_hold(root.path(), &expected_hold)?;
                    }
                    *expected_marker.borrow_mut() = std::fs::read(mh::hold_path(root.path(), 37))?;
                }
                let changing = head_reads.get() >= change_at;
                if changing && change.missing_change {
                    return Ok(None);
                }
                let state = if changing {
                    change.forge_state.unwrap_or(mh::ChangeState::Open)
                } else {
                    mh::ChangeState::Open
                };
                Ok(Some(mh::PinnedChange {
                    merged: state == mh::ChangeState::Merged,
                    state,
                    head_sha: if changing && change.missing_head {
                        None
                    } else {
                        Some(if changing && change.move_head {
                            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()
                        } else {
                            HEAD.into()
                        })
                    },
                    hold_label: true,
                }))
            },
            read_hold: &|| {
                hold_reads.set(hold_reads.get() + 1);
                if change.failed_read == Some("hold") {
                    return None;
                }
                if change.hold_field.is_some() {
                    // Exercise each exact-comparison field independently. The
                    // real reader separately normalizes unsupported schema/PR
                    // records to Unknown; that would mask a dropped field here.
                    serde_json::from_slice(&std::fs::read(mh::hold_path(root.path(), 37)).unwrap())
                        .ok()
                } else {
                    mh::read_hold_record(root.path(), 37)
                }
            },
            covered_specs: &|| {
                spec_reads.set(spec_reads.get() + 1);
                if spec_reads.get() >= change_at && change.failed_read == Some("specs") {
                    bail!("covered specs unavailable");
                }
                let mut current = store.clone();
                let mut current_ids = ids.clone();
                if spec_reads.get() >= change_at {
                    if change.fresh_corpus_id {
                        let mut other = f.req.clone();
                        other.spec_id = Some("TASK-40".into());
                        current.requirements.push(other);
                        current_ids.push("TASK-40".into());
                    }
                    match change.spec_field {
                        Some("tag") => {
                            current.requirements[0]
                                .tags
                                .insert("adr-75:authority".into());
                        }
                        Some("title") => {
                            current.requirements[0].title = "Credential storage".into()
                        }
                        Some("guided") => {
                            current.requirements[0].execution_mode =
                                Some(aida_core::ExecutionMode::Guided)
                        }
                        Some("operator") => {
                            current.requirements[0].execution_mode =
                                Some(aida_core::ExecutionMode::Operator)
                        }
                        Some("type") => {
                            current.requirements[0].req_type = aida_core::RequirementType::Epic
                        }
                        Some("keystone") => {
                            current.requirements[0].tags.insert("keystone".into());
                        }
                        Some("spec_id") => current.requirements[0].spec_id = Some("TASK-39".into()),
                        Some("agreed_id") => {
                            current.requirements[0].agreed_id = Some("TASK-39".into())
                        }
                        Some("collision-spec" | "collision-agreed" | "collision-mixed") => {
                            let mut other = aida_core::Requirement::new(
                                "Credential rotation".into(),
                                "collision fixture".into(),
                            );
                            other.execution_mode = Some(aida_core::ExecutionMode::Operator);
                            match change.spec_field {
                                Some("collision-spec") => other.spec_id = Some("task-37".into()),
                                Some("collision-agreed") => {
                                    current.requirements[0].spec_id = None;
                                    current.requirements[0].agreed_id = Some("TASK-37".into());
                                    other.agreed_id = Some("task-37".into());
                                }
                                Some("collision-mixed") => other.agreed_id = Some("task-37".into()),
                                _ => unreachable!(),
                            }
                            // Drive member stays first: first-match resolution would release.
                            current.requirements.push(other);
                        }
                        Some("missing") => current.requirements.clear(),
                        Some("empty") => current_ids.clear(),
                        Some("new-sensitive") => {
                            let mut other = f.req.clone();
                            other.spec_id = Some("TASK-38".into());
                            other.tags.insert("adr-75:authority".into());
                            current.requirements.push(other);
                            current_ids.push("TASK-38".into());
                        }
                        None => (),
                        Some(field) => panic!("unknown spec field {field}"),
                    }
                }
                Ok((current_ids, current))
            },
            read_body: &|key| {
                assert_eq!(key, "TASK-37");
                body_reads.set(body_reads.get() + 1);
                if body_reads.get() >= change_at && change.failed_read == Some("approval") {
                    bail!("approval unavailable");
                }
                let mut body: Value = serde_json::from_slice(&std::fs::read(&verdict_path)?)?;
                if body_reads.get() >= change_at {
                    if let Some(field) = change.approval_field {
                        match field {
                            "reviewed_sha" => {
                                body[field] =
                                    Value::String("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into())
                            }
                            "id" | "subject" | "session_id" | "recorded_by" => {
                                body["release_grant"][field] = Value::String("changed".into())
                            }
                            "recorded_at" => {
                                body[field] = Value::String("2026-10-14T01:00:00Z".into())
                            }
                            "rounds" => {
                                body[field][0]["recorded_at"] =
                                    Value::String("2026-10-14T01:00:03Z".into())
                            }
                            "round_verdict" => body["rounds"]
                                .as_array_mut()
                                .unwrap()
                                .push(serde_json::json!({"verdict":"unknown"})),
                            "missing_round_verdict" => body["rounds"]
                                .as_array_mut()
                                .unwrap()
                                .push(serde_json::json!({"recorded_at":"2026-10-14T01:00:01Z"})),
                            "unarmed" => {
                                body["rounds"][0]["verdict"] = Value::String("approved".into())
                            }
                            "missing_rounds" => {
                                body.as_object_mut().unwrap().remove("rounds");
                            }
                            "missing_timestamp" => {
                                body.as_object_mut().unwrap().remove("recorded_at");
                            }
                            "invalid_timestamp" => {
                                body["recorded_at"] = Value::String("invalid".into())
                            }
                            "invalid_round_timestamp" => {
                                body["rounds"][0]["recorded_at"] = Value::String("invalid".into())
                            }
                            "empty_recorder" => body["recorded_by"] = Value::String("".into()),
                            "missing_attestation" => {
                                body.as_object_mut().unwrap().remove("release_grant");
                            }
                            _ => panic!("unknown approval field {field}"),
                        }
                    }
                    if change.rewritten_approval {
                        body["verdict"] = Value::String("request-changes".into());
                    }
                    if change.changed_recorder {
                        body["recorded_by"] = Value::String("different reviewer".into());
                        body["release_grant"]["recorded_by"] = body["recorded_by"].clone();
                    }
                }
                Ok(body)
            },
            read_corpus: &|ids| {
                corpus_reads.set(corpus_reads.get() + 1);
                if change.failed_read == Some("corpus") {
                    bail!("corpus unavailable");
                }
                if change.after_audit && corpus_reads.get() == 2 {
                    if let Some(key) = change.corpus_key {
                        std::fs::write(
                            rv::verdict_path(root.path(), key),
                            serde_json::to_vec(&serde_json::json!({
                                "verdict": "request-changes", "reviewed_sha": HEAD
                            }))?,
                        )?;
                    }
                }
                mh::corpus_gate_bodies(&[root.path()], 37, ids).map_err(anyhow::Error::msg)
            },
            grant_check: &|id, subject| {
                // A changed artifact id/subject must be rejected by the shared
                // eligibility checks, not by an assertion in the injected reader.
                let _ = (id, subject);
                grant_reads.set(grant_reads.get() + 1);
                if change.rejected_grant && grant_reads.get() >= change_at {
                    sa::GrantCheck::Rejected {
                        id: f.grant.id.clone(),
                        seat: Some(f.grant.seat.clone()),
                        reason: sa::GrantRejection::Revoked { at: Utc::now() },
                    }
                } else {
                    let mut grant = f.grant.clone();
                    if grant_reads.get() >= change_at {
                        match change.grant_field {
                            Some("seat") => grant.seat = "implementer".into(),
                            Some("session_id") => grant.session_id = "other-session".into(),
                            Some("id") => grant.id = "other-id".into(),
                            Some("subject") => grant.subject = "other-subject".into(),
                            Some("absent") => return sa::GrantCheck::Absent,
                            None => (),
                            Some(field) => panic!("unknown grant field {field}"),
                        }
                    }
                    sa::GrantCheck::Valid(grant)
                }
            },
            comment: &|body| {
                comments.borrow_mut().push(body.to_string());
                Ok(())
            },
            labels: &|| {
                labels.borrow_mut().clear();
                Ok(())
            },
        },
    );
    let refusal = if let Some(read) = change.failed_read {
        Some(match read {
            "head" => "live head unavailable",
            "hold" => "hold changed",
            "approval" => "approval unavailable",
            "corpus" => "corpus unavailable",
            "specs" => "covered specs unavailable",
            _ => panic!("unknown read"),
        })
    } else if change.missing_change {
        Some("no forge change")
    } else if change.forge_state.is_some() {
        Some("change is not open")
    } else if change.missing_head {
        Some("current head could not be read")
    } else if change.move_head {
        Some("head moved")
    } else if change.before_eligibility && change.absorbed_kind.is_some() {
        Some("Rework-only")
    } else if change.stricter_hold
        || change.changed_rework
        || change.absorbed_kind.is_some()
        || change.hold_field.is_some()
    {
        Some("hold changed")
    } else if change.rejected_grant {
        Some("reviewer grant rejected")
    } else if change.rewritten_approval {
        Some("verdict is not APPROVED")
    } else if change.changed_recorder {
        Some("approval changed before release")
    } else if let Some(field) = change.approval_field {
        Some(match field {
            "reviewed_sha" => "stale",
            "id" | "subject" | "session_id" | "recorded_by" => "identity",
            "recorded_at" | "rounds" => "strictly newer",
            "round_verdict" | "missing_round_verdict" => "ambiguous",
            "unarmed" => "no earlier",
            "missing_rounds" => "missing arming",
            "missing_timestamp" => "missing approval timestamp",
            "invalid_timestamp" => "invalid approval timestamp",
            "invalid_round_timestamp" => "invalid arming refusal timestamp",
            "empty_recorder" => "no recording identity",
            "missing_attestation" => "provenance",
            _ => panic!(),
        })
    } else if let Some(field) = change.grant_field {
        Some(match field {
            "seat" => "implementer",
            "absent" => "no validated reviewer grant",
            _ => "identity",
        })
    } else if let Some(field) = change.spec_field {
        Some(match field {
            "collision-spec" | "collision-agreed" | "collision-mixed" => {
                "TASK-37 resolves to 2 requirements; exactly one required"
            }
            "guided" | "operator" => "guided/operator",
            "type" | "keystone" => "keystone",
            "empty" => "covered specs are missing",
            "missing" | "spec_id" | "agreed_id" => {
                "TASK-37 resolves to 0 requirements; exactly one required"
            }
            _ => "sensitive",
        })
    } else if change.corpus_key.is_some() || change.fresh_corpus_id {
        Some("verdict corpus still refuses this head")
    } else {
        None
    };
    if let Some(refusal) = refusal {
        let error = result.unwrap_err().to_string();
        assert!(error.contains(refusal), "{error}");
        let retained_hold: mh::MergeHoldRecord = if change.hold_field.is_some() {
            serde_json::from_slice(&std::fs::read(mh::hold_path(root.path(), 37)).unwrap()).unwrap()
        } else {
            mh::read_hold_record(root.path(), 37).unwrap()
        };
        assert_eq!(retained_hold.reason_kind, expected_hold.reason_kind);
        if change.stricter_hold {
            assert_eq!(retained_hold.absorbed.len(), 1);
            assert_eq!(
                retained_hold.absorbed[0].reason_kind,
                HoldReasonKind::Rework
            );
        } else {
            assert_eq!(retained_hold, expected_hold);
        }
        assert_eq!(
            std::fs::read(mh::hold_path(root.path(), 37)).unwrap(),
            *expected_marker.borrow()
        );
        if change.after_audit {
            // A real re-arm invalidates the prior clearance itself. Other races
            // retain the already-written authorization; none mirror the release.
            assert_eq!(
                mh::clearance_path(root.path(), 37).is_file(),
                !(change.stricter_hold || change.changed_rework)
            );
            assert_eq!(comments.borrow().len(), 1);
        } else {
            assert!(!mh::clearance_path(root.path(), 37).exists());
            assert!(comments.borrow().is_empty());
        }
        assert_eq!(*labels.borrow(), original_labels);
        // Behavioural release assertions above always precede read counters.
        if change.move_head || change.stricter_hold {
            assert_eq!(head_reads.get(), change_at);
            assert_eq!(hold_reads.get(), change_at - 1);
        }
    } else {
        assert!(result.unwrap());
        assert!(!mh::hold_path(root.path(), 37).exists());
        assert!(mh::clearance_path(root.path(), 37).is_file());
        assert_eq!(comments.borrow().len(), 1);
        assert!(labels.borrow().is_empty());
        assert_eq!(head_reads.get(), 3);
        assert_eq!(hold_reads.get(), 2);
        assert_eq!(corpus_reads.get(), 2);
        assert_eq!(spec_reads.get(), 3);
    }
}

#[test]
fn auto_release_current_head_and_unchanged_hold_releases() {
    auto_release_fixture(ReleaseChange::default());
}

#[test]
fn auto_release_refetches_moved_forge_head_before_any_release_effect() {
    auto_release_fixture(ReleaseChange {
        move_head: true,
        ..Default::default()
    });
}

#[test]
fn auto_release_rereads_stricter_hold_before_any_release_effect() {
    auto_release_fixture(ReleaseChange {
        stricter_hold: true,
        ..Default::default()
    });
}

// Every negative enters the same production validator before any release effect.
// trace:STORY-1687 | ai:codex
#[test]
fn auto_release_rereads_changed_rework_before_any_release_effect() {
    auto_release_fixture(ReleaseChange {
        changed_rework: true,
        ..Default::default()
    });
}

#[test]
fn auto_release_rechecks_revoked_grant_before_any_release_effect() {
    auto_release_fixture(ReleaseChange {
        rejected_grant: true,
        ..Default::default()
    });
}

#[test]
fn auto_release_rereads_rewritten_approval_before_any_release_effect() {
    auto_release_fixture(ReleaseChange {
        rewritten_approval: true,
        ..Default::default()
    });
}

#[test]
fn auto_release_rechecks_changed_approval_identity_before_any_release_effect() {
    auto_release_fixture(ReleaseChange {
        changed_recorder: true,
        ..Default::default()
    });
}

#[test]
fn auto_release_pr_corpus_refusal_preserves_every_release_effect() {
    auto_release_fixture(ReleaseChange {
        corpus_key: Some("PR-37"),
        ..Default::default()
    });
}

#[test]
fn auto_release_other_covered_spec_corpus_refusal_preserves_every_release_effect() {
    auto_release_fixture(ReleaseChange {
        corpus_key: Some("TASK-38"),
        ..Default::default()
    });
}

// The second production validation must also stop before labels/marker removal,
// even though clearance and its audit were already recorded by release_with.
// trace:STORY-1687 | ai:codex
#[test]
fn auto_release_after_audit_rechecks_every_condition_before_mirroring_release() {
    for change in [
        ReleaseChange {
            forge_state: Some(mh::ChangeState::ClosedUnmerged),
            ..Default::default()
        },
        ReleaseChange {
            missing_change: true,
            ..Default::default()
        },
        ReleaseChange {
            missing_head: true,
            ..Default::default()
        },
        ReleaseChange {
            move_head: true,
            ..Default::default()
        },
        ReleaseChange {
            stricter_hold: true,
            ..Default::default()
        },
        ReleaseChange {
            changed_rework: true,
            ..Default::default()
        },
        ReleaseChange {
            rejected_grant: true,
            ..Default::default()
        },
        ReleaseChange {
            rewritten_approval: true,
            ..Default::default()
        },
        ReleaseChange {
            changed_recorder: true,
            ..Default::default()
        },
        ReleaseChange {
            corpus_key: Some("PR-37"),
            ..Default::default()
        },
        ReleaseChange {
            corpus_key: Some("TASK-38"),
            ..Default::default()
        },
    ] {
        auto_release_fixture(ReleaseChange {
            after_audit: true,
            ..change
        });
    }
}

#[test]
fn auto_release_failed_release_time_reads_preserve_every_release_effect() {
    for read in ["head", "hold", "approval", "corpus", "specs"] {
        auto_release_fixture(ReleaseChange {
            failed_read: Some(read),
            ..Default::default()
        });
    }
}

// The forge may close/merge the change, stop recognizing it, or omit its head
// after eligibility. These checks run inside the production validator.
// trace:STORY-1687 | ai:codex
#[test]
fn auto_release_nonopen_forge_change_preserves_every_release_effect() {
    for state in [
        mh::ChangeState::ClosedUnmerged,
        mh::ChangeState::Merged,
        mh::ChangeState::Unknown,
    ] {
        auto_release_fixture(ReleaseChange {
            forge_state: Some(state),
            ..Default::default()
        });
    }
}

#[test]
fn auto_release_missing_forge_change_preserves_every_release_effect() {
    auto_release_fixture(ReleaseChange {
        missing_change: true,
        ..Default::default()
    });
}

#[test]
fn auto_release_missing_forge_head_preserves_every_release_effect() {
    auto_release_fixture(ReleaseChange {
        missing_head: true,
        ..Default::default()
    });
}

// trace:STORY-1687 | ai:codex
fn absorbed(kind: HoldReasonKind) -> AbsorbedHold {
    AbsorbedHold {
        reason_kind: kind,
        detail: "absorbed authorization".into(),
        recused_principals: vec![mh::PrincipalIdentity::human("old")],
        target_head_sha: Some(HEAD.into()),
        spec: Some("TASK-37".into()),
        placed_by: Some("old reviewer".into()),
        verdict_ref: Some(mh::VerdictRef::new(
            "TASK-37",
            Some(37),
            Some(HEAD.into()),
            Some("old reviewer".into()),
        )),
    }
}

fn changed_hold_value(field: &str) -> Value {
    match field {
        "schema_version" => serde_json::json!(99),
        "pr" => serde_json::json!(38),
        "reason_kind" => serde_json::json!("decision"),
        "detail" => serde_json::json!("changed detail"),
        "recused_principals" | "routed_to" => {
            serde_json::to_value(vec![mh::PrincipalIdentity::human("operator")]).unwrap()
        }
        "routing_state" => serde_json::json!("routed"),
        "target_head_sha" => serde_json::json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        "label_state" => serde_json::json!("pending"),
        "legacy" => serde_json::json!(true),
        "verdict_ref" => serde_json::to_value(mh::VerdictRef::new(
            "TASK-38",
            Some(38),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()),
            Some("other reviewer".into()),
        ))
        .unwrap(),
        "release_condition" => serde_json::json!("human only"),
        "spec" => serde_json::json!("TASK-38"),
        "placed_by" => serde_json::json!("other reviewer"),
        "absorbed" => serde_json::to_value(vec![absorbed(HoldReasonKind::Rework)]).unwrap(),
        _ => panic!("unknown hold field {field}"),
    }
}

#[test]
fn auto_release_non_rework_absorption_at_each_phase_preserves_release_effects() {
    for kind in [
        HoldReasonKind::Supervision,
        HoldReasonKind::Decision,
        HoldReasonKind::Recusal,
        HoldReasonKind::Unknown,
    ] {
        for (before_eligibility, after_audit) in [(true, false), (false, false), (false, true)] {
            auto_release_fixture(ReleaseChange {
                absorbed_kind: Some(kind),
                before_eligibility,
                after_audit,
                ..Default::default()
            });
        }
    }
}

#[test]
fn auto_release_every_hold_record_field_change_preserves_release_effects() {
    for field in [
        "schema_version",
        "pr",
        "reason_kind",
        "detail",
        "recused_principals",
        "routed_to",
        "routing_state",
        "target_head_sha",
        "label_state",
        "legacy",
        "verdict_ref",
        "release_condition",
        "spec",
        "placed_by",
        "absorbed",
    ] {
        for after_audit in [false, true] {
            auto_release_fixture(ReleaseChange {
                hold_field: Some(field),
                after_audit,
                ..Default::default()
            });
        }
    }
}

macro_rules! approval_field_test {
    ($name:ident, $field:literal) => {
        #[test]
        fn $name() {
            for after_audit in [false, true] {
                auto_release_fixture(ReleaseChange {
                    approval_field: Some($field),
                    after_audit,
                    ..Default::default()
                });
            }
        }
    };
}
approval_field_test!(auto_release_changed_reviewed_sha_stays_held, "reviewed_sha");
approval_field_test!(auto_release_changed_release_grant_id_stays_held, "id");
approval_field_test!(
    auto_release_changed_release_grant_subject_stays_held,
    "subject"
);
approval_field_test!(
    auto_release_changed_release_grant_session_stays_held,
    "session_id"
);
approval_field_test!(
    auto_release_changed_release_grant_recorder_stays_held,
    "recorded_by"
);
approval_field_test!(auto_release_changed_recorded_at_stays_held, "recorded_at");
approval_field_test!(auto_release_changed_rounds_stays_held, "rounds");

#[test]
fn auto_release_changed_approval_provenance_stays_held() {
    for field in [
        "round_verdict",
        "unarmed",
        "missing_rounds",
        "missing_timestamp",
        "invalid_timestamp",
        "invalid_round_timestamp",
        "empty_recorder",
        "missing_attestation",
    ] {
        for after_audit in [false, true] {
            auto_release_fixture(ReleaseChange {
                approval_field: Some(field),
                after_audit,
                ..Default::default()
            });
        }
    }
}

macro_rules! grant_field_test {
    ($name:ident, $field:literal) => {
        #[test]
        fn $name() {
            for after_audit in [false, true] {
                auto_release_fixture(ReleaseChange {
                    grant_field: Some($field),
                    after_audit,
                    ..Default::default()
                });
            }
        }
    };
}
grant_field_test!(auto_release_changed_fresh_grant_seat_stays_held, "seat");
grant_field_test!(
    auto_release_changed_fresh_grant_session_stays_held,
    "session_id"
);
grant_field_test!(auto_release_changed_fresh_grant_id_stays_held, "id");
grant_field_test!(
    auto_release_changed_fresh_grant_subject_stays_held,
    "subject"
);
grant_field_test!(auto_release_absent_fresh_grant_stays_held, "absent");

#[test]
fn auto_release_rereads_spec_metadata_and_execution_mode_at_each_release_phase() {
    for field in [
        "tag",
        "title",
        "guided",
        "operator",
        "type",
        "keystone",
        "missing",
        "empty",
        "new-sensitive",
        "spec_id",
        "agreed_id",
    ] {
        for after_audit in [false, true] {
            auto_release_fixture(ReleaseChange {
                spec_field: Some(field),
                after_audit,
                ..Default::default()
            });
        }
    }
}

// Exact identity recursively includes the fields of retained holds, verdict refs
// and principal identities. Change only one leaf at a time.
// trace:STORY-1687 | ai:codex
fn changed_nested_hold_value(path: &str) -> Value {
    match path.rsplit('/').next().unwrap() {
        "reason_kind" => serde_json::json!("supervision"),
        "detail" => serde_json::json!("changed absorbed detail"),
        "recused_principals" => {
            serde_json::to_value(vec![mh::PrincipalIdentity::human("new")]).unwrap()
        }
        "target_head_sha" | "reviewed_sha" => {
            serde_json::json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        }
        "spec" | "key" => serde_json::json!("TASK-38"),
        "placed_by" | "recorded_by" | "principal_id" => serde_json::json!("new reviewer"),
        "verdict_ref" => serde_json::to_value(mh::VerdictRef::new(
            "TASK-38",
            Some(38),
            Some(HEAD.into()),
            Some("new reviewer".into()),
        ))
        .unwrap(),
        "pr" => serde_json::json!(38),
        "principal_kind" => serde_json::json!("operator"),
        "identity_status" => serde_json::json!("unresolved"),
        _ => panic!("unknown path {path}"),
    }
}

#[test]
fn auto_release_every_nested_hold_field_change_preserves_release_effects() {
    for path in [
        "/absorbed/0/reason_kind",
        "/absorbed/0/detail",
        "/absorbed/0/recused_principals",
        "/absorbed/0/target_head_sha",
        "/absorbed/0/spec",
        "/absorbed/0/placed_by",
        "/absorbed/0/verdict_ref",
        "/absorbed/0/recused_principals/0/principal_kind",
        "/absorbed/0/recused_principals/0/principal_id",
        "/absorbed/0/recused_principals/0/identity_status",
        "/absorbed/0/verdict_ref/key",
        "/absorbed/0/verdict_ref/pr",
        "/absorbed/0/verdict_ref/reviewed_sha",
        "/absorbed/0/verdict_ref/recorded_by",
        "/verdict_ref/key",
        "/verdict_ref/pr",
        "/verdict_ref/reviewed_sha",
        "/verdict_ref/recorded_by",
        "/recused_principals/0/principal_kind",
        "/recused_principals/0/principal_id",
        "/recused_principals/0/identity_status",
        "/routed_to/0/principal_kind",
        "/routed_to/0/principal_id",
        "/routed_to/0/identity_status",
    ] {
        for after_audit in [false, true] {
            auto_release_fixture(ReleaseChange {
                hold_field: Some(path),
                after_audit,
                ..Default::default()
            });
        }
    }
}

// trace:STORY-1687 | ai:codex
#[test]
fn malformed_matching_heads_cannot_authorize_release() {
    for head in [
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "gggggggggggggggggggggggggggggggggggggggg",
    ] {
        let mut f = Fixture::new();
        f.body["reviewed_sha"] = Value::String(head.into());
        let error = eligible(
            &f.hold,
            &f.body,
            head,
            &[Some(&f.req)],
            &sa::GrantCheck::Valid(f.grant.clone()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("unverifiable head"));
    }
}

// Even a matching stamp cannot authorize a blank recording principal.
// trace:STORY-1687 | ai:codex
#[test]
fn matching_blank_recording_identity_cannot_authorize_release() {
    for identity in ["", "   "] {
        let mut f = Fixture::new();
        f.body["recorded_by"] = Value::String(identity.into());
        f.body["release_grant"]["recorded_by"] = Value::String(identity.into());
        f.refused("no recording identity");
    }
}

// Structural provenance refusals have individual behavioural witnesses too.
// trace:STORY-1687 | ai:codex
approval_field_test!(
    auto_release_missing_release_grant_stays_held,
    "missing_attestation"
);
approval_field_test!(auto_release_missing_rounds_stays_held, "missing_rounds");

approval_field_test!(
    auto_release_missing_round_verdict_stays_held,
    "missing_round_verdict"
);

// trace:STORY-1687 | ai:codex
#[test]
fn auto_release_exactly_one_requirement_per_id_at_each_phase() {
    for field in [
        "collision-spec",
        "collision-agreed",
        "collision-mixed",
        "missing",
    ] {
        for (before_eligibility, after_audit) in [(true, false), (false, false), (false, true)] {
            auto_release_fixture(ReleaseChange {
                spec_field: Some(field),
                before_eligibility,
                after_audit,
                ..Default::default()
            });
        }
    }
}

// trace:STORY-1687 | ai:codex
#[test]
fn auto_release_fresh_covered_id_corpus_refusal_at_each_release_phase() {
    for after_audit in [false, true] {
        auto_release_fixture(ReleaseChange {
            fresh_corpus_id: true,
            after_audit,
            ..Default::default()
        });
    }
}

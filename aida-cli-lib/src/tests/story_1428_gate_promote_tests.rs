// STORY-1428: handler tests for `aida findings promote --to gate`.
// trace:STORY-1428 | ai:claude
use super::*;
use aida_core::db::DatabaseBackend;
use aida_core::CachedGitBackend;

const FULL: GateAuthority = GateAuthority {
    advisor: true,
    dispatch: true,
};
const NONE: GateAuthority = GateAuthority {
    advisor: false,
    dispatch: false,
};

struct Fixture {
    _dir: tempfile::TempDir,
    store: std::path::PathBuf,
    backend: CachedGitBackend,
}

fn fixture(recurrence: u32) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let backend =
        CachedGitBackend::open(&store, &dir.path().join(".aida").join("cache.db")).unwrap();
    let mut f = Requirement::new("ETXTBSY on fixture".into(), "seen again".into());
    f.spec_id = Some("TASK-900".into());
    f.tags.insert("from-advisor:general".into());
    if recurrence > 1 {
        f.tags.insert(format!("recurrence:{recurrence}"));
    }
    backend.add_requirement(f).unwrap();
    Fixture {
        _dir: dir,
        store,
        backend,
    }
}

fn finding(fx: &Fixture) -> Requirement {
    fx.backend
        .get_requirement_unambiguous("TASK-900")
        .unwrap()
        .unwrap()
}

fn promote(fx: &Fixture, force: bool, auth: GateAuthority) -> Result<GatePromoteOutcome> {
    promote_finding_to_gate(
        &fx.backend,
        &fx.store,
        finding(fx),
        "TASK-900",
        findings::GateScreen::Mechanical,
        None,
        None,
        force,
        auth,
    )
}

fn gate_tasks(fx: &Fixture) -> Vec<Requirement> {
    fx.backend
        .load()
        .unwrap()
        .requirements
        .into_iter()
        .filter(|r| r.tags.contains(findings::GATE_CANDIDATE_TAG))
        .collect()
}

#[test]
fn gate_route_refused_below_threshold() {
    let fx = fixture(2);
    let err = promote(&fx, false, FULL).unwrap_err().to_string();
    assert!(err.contains("below the gate threshold"), "{err}");
    assert!(gate_tasks(&fx).is_empty());
    assert!(
        findings::gate_decision(&finding(&fx).tags.iter().cloned().collect::<Vec<_>>()).is_none()
    );
}

#[test]
fn gate_route_links_finding_to_gate_task() {
    let _user = crate::test_env::EnvVarGuard::set("AIDA_USER", "story1428-link-user");
    let fx = fixture(3);
    let out = promote(&fx, false, FULL).unwrap();
    let gates = gate_tasks(&fx);
    assert_eq!(gates.len(), 1);
    let gate = &gates[0];
    let gate_id = gate.spec_id.clone().unwrap();
    let f = finding(&fx);
    let tags: Vec<String> = f.tags.iter().cloned().collect();
    assert_eq!(findings::gated_by(&tags), Some(gate_id.as_str()));
    assert_eq!(
        findings::gate_decision(&tags),
        Some(findings::GateScreen::Mechanical)
    );
    // Linked both ways.
    assert!(gate.relationships.iter().any(|r| r.target_id == f.id));
    assert!(f.relationships.iter().any(|r| r.target_id == gate.id));
    // The finding itself is not left Approved outside any queue.
    assert_eq!(f.status, RequirementStatus::Draft);
    // Full authority: Approved and queued.
    assert_eq!(gate.status, RequirementStatus::Approved);
    match out {
        GatePromoteOutcome::Gated {
            queued_for, reused, ..
        } => {
            assert_eq!(queued_for.as_deref(), Some("implementer"));
            assert!(!reused);
        }
        other => panic!("unexpected {other:?}"),
    }
    let entries = Storage::new(&fx.store)
        .queue_list(&current_user_id(None), false)
        .unwrap();
    assert!(entries.iter().any(|e| e.requirement_id == gate.id));
}

#[test]
fn gate_route_does_not_refile_on_force_retry() {
    let _user = crate::test_env::EnvVarGuard::set("AIDA_USER", "story1428-refile-user");
    let fx = fixture(3);
    promote(&fx, false, NONE).unwrap();
    // Without --force the settled question is not re-opened.
    let err = promote(&fx, false, NONE).unwrap_err().to_string();
    assert!(err.contains("already settled"), "{err}");
    // With --force the existing gate task is reused, not duplicated.
    let out = promote(&fx, true, NONE).unwrap();
    assert!(matches!(
        out,
        GatePromoteOutcome::Gated { reused: true, .. }
    ));
    assert_eq!(gate_tasks(&fx).len(), 1);
}

#[test]
fn gate_route_reuses_task_when_finding_link_was_lost() {
    // A retry after the task was written but before the finding was tagged
    // `gated-by:` finds the task by its reference to the finding.
    let _user = crate::test_env::EnvVarGuard::set("AIDA_USER", "story1428-lost-link-user");
    let fx = fixture(3);
    promote(&fx, false, NONE).unwrap();
    let mut f = finding(&fx);
    f.tags.retain(|t| !t.starts_with(findings::GATED_BY_PREFIX));
    fx.backend.update_requirement(&f).unwrap();
    promote(&fx, true, NONE).unwrap();
    assert_eq!(gate_tasks(&fx).len(), 1);
}

#[test]
fn gate_route_without_authority_files_draft_and_does_not_queue() {
    let _user = crate::test_env::EnvVarGuard::set("AIDA_USER", "story1428-noauth-user");
    let fx = fixture(3);
    let out = promote(&fx, false, NONE).unwrap();
    let gates = gate_tasks(&fx);
    assert_eq!(gates[0].status, RequirementStatus::Draft);
    match &out {
        GatePromoteOutcome::Gated { queued_for, .. } => assert_eq!(queued_for, &None),
        other => panic!("unexpected {other:?}"),
    }
    assert!(out.message().contains("not queued"), "{}", out.message());
    let entries = Storage::new(&fx.store)
        .queue_list(&current_user_id(None), false)
        .unwrap();
    assert!(entries.iter().all(|e| e.requirement_id != gates[0].id));
}

#[test]
fn gate_route_advisor_without_dispatch_files_approved_but_does_not_queue() {
    let _user = crate::test_env::EnvVarGuard::set("AIDA_USER", "story1428-nodispatch-user");
    let fx = fixture(3);
    let auth = GateAuthority {
        advisor: true,
        dispatch: false,
    };
    let out = promote(&fx, false, auth).unwrap();
    assert!(matches!(
        out,
        GatePromoteOutcome::Gated {
            queued_for: None,
            ..
        }
    ));
}

#[test]
fn gate_route_prose_files_nothing() {
    let fx = fixture(3);
    let out = promote_finding_to_gate(
        &fx.backend,
        &fx.store,
        finding(&fx),
        "TASK-900",
        findings::GateScreen::Prose,
        Some("no mechanical signal"),
        None,
        false,
        FULL,
    )
    .unwrap();
    assert!(matches!(out, GatePromoteOutcome::StaysProse { .. }));
    assert!(gate_tasks(&fx).is_empty());
    let tags: Vec<String> = finding(&fx).tags.iter().cloned().collect();
    assert_eq!(
        findings::gate_decision(&tags),
        Some(findings::GateScreen::Prose)
    );
}

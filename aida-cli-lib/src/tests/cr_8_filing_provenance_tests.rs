// CR-8: regression tests for filing provenance on the CLI finding-creation
// paths that persist through `update_atomically` and then rewrite the object
// from the in-memory copy (`object_store::write_object`). Before the fix the
// rewrite dropped the stamp the full-store save had just written.
// trace:CR-8 | ai:claude
use super::*;
use aida_core::db::DatabaseBackend;
use aida_core::CachedGitBackend;

struct Fixture {
    _dir: tempfile::TempDir,
    store: std::path::PathBuf,
    backend: CachedGitBackend,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let backend =
        CachedGitBackend::open(&store, &dir.path().join(".aida").join("cache.db")).unwrap();
    Fixture {
        _dir: dir,
        store,
        backend,
    }
}

/// Read every object straight from the YAML files on disk (the substrate the
/// buggy rewrite clobbered), not from any in-memory copy.
fn on_disk(fx: &Fixture) -> Vec<Requirement> {
    aida_core::object_store::load_all_objects(&fx.store.join("objects")).unwrap()
}

#[test]
fn findings_add_persists_filing_provenance() {
    let fx = fixture();
    handle_findings_add(
        &fx.backend,
        &fx.store,
        "saw a flaky fixture",
        "flake",
        Some("CR-8 regression finding"),
        Some("minor"),
        &[],
        None,
    )
    .unwrap();
    let reqs = on_disk(&fx);
    let finding = reqs
        .iter()
        .find(|r| r.title == "CR-8 regression finding")
        .expect("finding written");
    let stamp = finding
        .filed_at
        .as_ref()
        .expect("findings add must persist filing provenance on disk");
    assert!(stamp.aida_version.is_some());
}

#[test]
fn promote_to_gate_persists_filing_provenance() {
    let fx = fixture();
    let mut f = Requirement::new("ETXTBSY on fixture".into(), "seen again".into());
    f.spec_id = Some("TASK-900".into());
    f.tags.insert("from-advisor:general".into());
    f.tags.insert("recurrence:3".into());
    fx.backend.add_requirement(f).unwrap();
    let finding = fx
        .backend
        .get_requirement_unambiguous("TASK-900")
        .unwrap()
        .unwrap();
    promote_finding_to_gate(
        &fx.backend,
        &fx.store,
        finding,
        "TASK-900",
        findings::GateScreen::Mechanical,
        None,
        None,
        true,
        GateAuthority {
            advisor: true,
            dispatch: true,
        },
    )
    .unwrap();
    let reqs = on_disk(&fx);
    let gate = reqs
        .iter()
        .find(|r| r.tags.contains(findings::GATE_CANDIDATE_TAG))
        .expect("gate task written");
    assert!(
        gate.filed_at.is_some(),
        "promote-to-gate must persist filing provenance on disk"
    );
}

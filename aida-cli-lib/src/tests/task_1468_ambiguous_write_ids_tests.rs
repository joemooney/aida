//! TASK-1468: write paths refuse an id that names two requirements (a
//! merge-gate agreed_id colliding with another spec's native spec_id); the
//! pull auto-bump skips an ambiguous commit-trailer id instead of failing.
// trace:TASK-1468 | ai:claude

use super::{
    add_blocked_by_edge, ensure_parent_edge_from_tag, parse_requirement_id, trailer_spec_or_skip,
};
use aida_core::db::DatabaseBackend;
use aida_core::id_collisions::AmbiguousIdError;
use aida_core::models::{Requirement, RequirementsStore};
use aida_core::CachedGitBackend;
use tempfile::tempdir;

fn req(spec_id: &str, agreed_id: Option<&str>, title: &str) -> Requirement {
    let mut r = Requirement::new(title.into(), "desc".into());
    r.spec_id = Some(spec_id.into());
    r.agreed_id = agreed_id.map(str::to_string);
    r
}

/// `BUG-34` names the native fixture AND the agreed-id holder `BUG-2-081`.
fn colliding_store() -> (RequirementsStore, Requirement, Requirement) {
    let fixture = req("BUG-34", None, "fixture");
    let real = req("BUG-2-081", Some("BUG-34"), "real");
    let mut store = RequirementsStore::new();
    store.requirements = vec![fixture.clone(), real.clone()];
    (store, fixture, real)
}

#[test]
fn trailer_with_an_ambiguous_id_is_skipped_not_guessed() {
    let (store, _fixture, real) = colliding_store();
    assert!(trailer_spec_or_skip(&store, "BUG-34", "0123456789abcdef").is_none());
    // The unambiguous handle still resolves, so the pull keeps going.
    let found = trailer_spec_or_skip(&store, "BUG-2-081", "0123456789abcdef").unwrap();
    assert_eq!(found.id, real.id);
    assert!(trailer_spec_or_skip(&store, "BUG-99", "abc").is_none());
}

#[test]
fn legacy_id_parser_refuses_an_ambiguous_id() {
    let (store, fixture, _real) = colliding_store();
    let err = parse_requirement_id("bug-34", &store).unwrap_err();
    assert!(err.downcast_ref::<AmbiguousIdError>().is_some(), "{err:#}");
    // A UUID reaches the native owner.
    let uuid = parse_requirement_id(&fixture.id.to_string(), &store).unwrap();
    assert_eq!(uuid, fixture.id);
}

#[test]
fn cli_edge_writers_refuse_an_ambiguous_id() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join("store");
    let cache_path = dir.path().join(".aida").join("cache.db");
    std::fs::create_dir_all(&store_root).unwrap();
    let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
    backend
        .add_requirement(req("BUG-34", None, "fixture"))
        .unwrap();
    backend
        .add_requirement(req("BUG-2-081", Some("BUG-34"), "real"))
        .unwrap();
    let mut child = req("TASK-7", None, "child");
    child.tags.insert("parent:BUG-34".to_string());
    backend.add_requirement(child).unwrap();

    let err = add_blocked_by_edge(&backend, "TASK-7", "BUG-34").unwrap_err();
    assert!(err.downcast_ref::<AmbiguousIdError>().is_some(), "{err:#}");
    let err = ensure_parent_edge_from_tag(&backend, "TASK-7").unwrap_err();
    assert!(err.downcast_ref::<AmbiguousIdError>().is_some(), "{err:#}");

    // Nothing was written to the child.
    let child = backend
        .get_requirement_by_spec_id("TASK-7")
        .unwrap()
        .unwrap();
    assert!(child.relationships.is_empty());
}

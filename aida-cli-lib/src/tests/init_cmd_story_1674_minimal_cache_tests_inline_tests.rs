use super::ensure_minimal_cache;
use aida_core::db::{DatabaseBackend, SqliteBackend};

/// The minimal-footprint cache is a derived copy of the source store: no
/// import write event (or any other stamp) is added to its rows.
// trace:STORY-1674 | ai:claude
#[test]
fn story1674_minimal_cache_is_copied_unstamped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut store = aida_core::RequirementsStore::new();
    let mut req = aida_core::Requirement::new("source".into(), "body".into());
    req.spec_id = Some("TASK-1".into());
    store.requirements.push(req);
    std::fs::write(
        root.join("requirements.yaml"),
        serde_yaml::to_string(&store).unwrap(),
    )
    .unwrap();
    ensure_minimal_cache(root, std::path::Path::new("requirements.yaml"));
    let cache = SqliteBackend::new(aida_core::db::project_cache_path(&root)).unwrap();
    let copied = cache.load().unwrap();
    let row = copied
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some("TASK-1"))
        .expect("the cache holds the source row");
    assert!(row.history.is_empty(), "no write event: {:?}", row.history);
    assert!(row.filed_at.is_none(), "no invented filing provenance");
}

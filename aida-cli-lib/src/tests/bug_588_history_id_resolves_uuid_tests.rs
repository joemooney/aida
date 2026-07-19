use super::*;
use aida_core::db::{DatabaseBackend, GitBackend};
use aida_core::{Requirement, RequirementsStore};

/// BUG-588: `aida history --id <X>` keys on spec_id (the orphan-branch
/// event decoder tags every event with the YAML's spec_id). `aida show`
/// prints the raw UUID, so a user copying it into `--id <uuid>` got an
/// empty "(no recent activity)" — the filter never matched. The resolver
/// must turn a UUID into its canonical spec_id so the documented
/// invocation works.
///
/// Fail-old/pass-new: before the fix the UUID was passed through verbatim
/// (the filter then compared a UUID against spec_ids and matched nothing);
/// the new resolver returns the spec_id. A non-UUID (real spec_id) and an
/// unknown UUID both pass through unchanged.
#[test]
fn resolve_history_id_filter_maps_uuid_to_spec_id() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let backend = GitBackend::new(&root).unwrap();

    let mut req = Requirement::new("Spec under test".into(), "desc".into());
    req.spec_id = Some("TASK-42".into());
    let uuid = req.id;
    let mut store = RequirementsStore::new();
    store.requirements.push(req);
    backend.save(&store).unwrap();

    // The bug's exact reproduction: feeding the UUID resolves to the
    // spec_id the event decoder keys on.
    assert_eq!(
        resolve_history_id_filter(&backend, &uuid.to_string()),
        "TASK-42",
        "a UUID must resolve to its canonical spec_id (BUG-588)"
    );

    // A spec_id argument passes through untouched (no double-resolution).
    assert_eq!(
        resolve_history_id_filter(&backend, "TASK-42"),
        "TASK-42",
        "a spec_id argument must pass through unchanged"
    );

    // An unknown UUID has nothing to resolve to → pass through verbatim
    // (it will simply match no events, which is the honest answer).
    let unknown = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        resolve_history_id_filter(&backend, &unknown),
        unknown,
        "an unresolvable UUID must pass through unchanged"
    );
}

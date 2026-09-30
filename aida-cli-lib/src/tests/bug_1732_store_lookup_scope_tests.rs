//! BUG-1732: `load_store_for_lookup` must be pure with respect to its
//! `project_root`, so a project-scoped read can never surface another
//! project's requirements.
//!
//! The reported symptom was `aida awaiting` listing six In-Progress specs from
//! an unrelated project in its orphaned-in-progress channel. The path is
//! `collect_awaiting_report` → `gather_running_work(project_root)` →
//! `running_work_spec_index(project_root)` → `load_store_for_lookup`, whose
//! legacy arm resolved the store off the PROCESS CWD (and an ambient
//! `REQ_DB_NAME`) rather than the root it was handed.
// trace:BUG-1732 | ai:claude

use super::*;

/// Write a legacy YAML store into `dir` holding one requirement with `spec_id`.
fn seed_legacy_store(dir: &std::path::Path, spec_id: &str) {
    let mut store = aida_core::RequirementsStore::default();
    let mut r = Requirement::new(format!("{spec_id} title"), "desc".into());
    r.spec_id = Some(spec_id.to_string());
    r.status = aida_core::RequirementStatus::InProgress;
    store.requirements.push(r);
    Storage::new(dir.join("requirements.yaml"))
        .save(&store)
        .unwrap();
}

/// A legacy store IS read when it sits in the project root, and the store that
/// comes back is that root's own — not a different root's that also holds one.
/// Host-independent: both stores are created by the test.
#[test]
fn legacy_lookup_reads_the_project_roots_own_store_not_a_sibling_roots() {
    let scoped = tempfile::TempDir::new().unwrap();
    let foreign = tempfile::TempDir::new().unwrap();
    seed_legacy_store(scoped.path(), "BUG-17320");
    seed_legacy_store(foreign.path(), "FR-0281");

    let store = load_store_for_lookup(scoped.path())
        .expect("a legacy store in the project root must still be read");
    let ids: Vec<&str> = store
        .requirements
        .iter()
        .filter_map(|r| r.spec_id.as_deref())
        .collect();
    assert_eq!(
        ids,
        ["BUG-17320"],
        "the lookup must return the requested root's store, not a sibling's"
    );
}

/// The regression itself: a project root with no store of its own yields
/// nothing, whatever legacy store happens to be reachable from the process
/// CWD. Before the fix this returned the CWD's store — on the host that filed
/// BUG-1732, `/home/joe/ai/aida/requirements.db`, a stale March-2026 legacy
/// store whose eight In-Progress specs (`FR-0281`, `STORY-0326`, …) are exactly
/// what `aida awaiting` reported.
///
/// NOTE ON COVERAGE: this and the sibling-discrimination test above both bite
/// only where the process CWD really does hold a legacy store, which is the
/// filing host and not CI (`requirements.db` is untracked, which is precisely
/// why the pre-existing
/// `story_465_awaiting_report_tests::empty_state_produces_hidden_report` failed
/// locally and passed on CI). Asserting it on every host would mean mutating
/// the process-wide CWD, which is unsound under a threaded test harness. The
/// argument-level pin below runs everywhere but covers a strictly smaller
/// claim — the resolver's contract, not this function's wiring to it — so on a
/// clean host these two pass trivially rather than being witnessed.
#[test]
fn a_project_root_without_a_store_yields_nothing() {
    let bare = tempfile::TempDir::new().unwrap();
    assert!(
        load_store_for_lookup(bare.path()).is_none(),
        "a root with neither a distributed nor a legacy store must read as absent"
    );
}

/// The host-independent half: pin the ARGUMENTS the legacy arm hands the
/// shared resolver. `project_root` stands in for the cwd, and both ambient
/// inputs (`-p` and `REQ_DB_NAME`) are `None`, so no directory other than
/// `project_root` can be adopted. Passing a directory with no store must be a
/// refusal rather than a fallback to anywhere else.
#[test]
fn the_legacy_resolver_is_given_the_project_root_and_no_ambient_inputs() {
    let seeded = tempfile::TempDir::new().unwrap();
    let bare = tempfile::TempDir::new().unwrap();
    seed_legacy_store(seeded.path(), "BUG-17321");

    let inert_registry = std::path::Path::new("");
    assert_eq!(
        aida_core::resolve_requirements_path_in(seeded.path(), None, None, inert_registry)
            .expect("a store local to the given root resolves"),
        seeded.path().join("requirements.yaml"),
    );
    assert!(
        aida_core::resolve_requirements_path_in(bare.path(), None, None, inert_registry).is_err(),
        "with no local store and no explicit project, resolution must refuse \
         rather than adopt the cwd, REQ_DB_NAME, or the registry default"
    );
}

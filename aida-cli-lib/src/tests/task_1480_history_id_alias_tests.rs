//! TASK-1480: `aida history <SPEC-ID>` is a positional alias for
//! `aida history --id <SPEC-ID>`; both funnel through
//! `resolve_history_id_filter`, which is also the "invalid or ambiguous IDs
//! get a clear error" gate for the single-spec history views. These tests
//! exercise that resolver directly (the CLI-parsing half of the alias lives
//! in `cli.rs`'s `history_positional_spec_id_parses`).
// trace:TASK-1480 | ai:claude

use super::*;
use aida_core::db::DatabaseBackend;
use aida_core::id_collisions::AmbiguousIdError;
use aida_core::models::Requirement;
use aida_core::CachedGitBackend;
use tempfile::tempdir;

fn req(spec_id: &str, agreed_id: Option<&str>, title: &str) -> Requirement {
    let mut r = Requirement::new(title.into(), "desc".into());
    r.spec_id = Some(spec_id.into());
    r.agreed_id = agreed_id.map(str::to_string);
    r
}

fn open_backend() -> (tempfile::TempDir, CachedGitBackend) {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join("store");
    let cache_path = dir.path().join(".aida").join("cache.db");
    std::fs::create_dir_all(&store_root).unwrap();
    let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
    (dir, backend)
}

/// A string that can't possibly be a spec id (no `TYPE-SEQ` shape, not a
/// UUID) is refused immediately with the BUG-599-style format hint, not a
/// generic "no recent activity."
#[test]
fn resolve_history_id_filter_refuses_malformed_id() {
    let (_dir, backend) = open_backend();
    let err = resolve_history_id_filter(&backend, "not-a-real-id").unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not a valid spec ID"),
        "expected the format-hint message, got: {msg}"
    );
}

/// An id that resolves to more than one requirement (a merge-gate
/// agreed_id colliding with another spec's native spec_id, BUG-1535's
/// scenario) is refused with `AmbiguousIdError`, not guessed at.
#[test]
fn resolve_history_id_filter_refuses_ambiguous_id() {
    let (_dir, backend) = open_backend();
    backend
        .add_requirement(req("BUG-34", None, "fixture"))
        .unwrap();
    backend
        .add_requirement(req("BUG-2-081", Some("BUG-34"), "real"))
        .unwrap();

    let err = resolve_history_id_filter(&backend, "BUG-34").unwrap_err();
    assert!(
        err.downcast_ref::<AmbiguousIdError>().is_some(),
        "expected an AmbiguousIdError, got: {err:#}"
    );

    // The unambiguous handle (the agreed-id holder's own native spec_id)
    // still resolves cleanly.
    assert_eq!(
        resolve_history_id_filter(&backend, "BUG-2-081").unwrap(),
        "BUG-2-081"
    );
}

/// A live requirement resolves by spec_id, by its agreed_id, and by its
/// UUID — all to the same canonical spec_id the event decoder keys on.
#[test]
fn resolve_history_id_filter_resolves_live_requirement_every_way() {
    let (_dir, backend) = open_backend();
    let r = req("TASK-9", None, "live spec");
    let uuid = r.id;
    backend.add_requirement(r).unwrap();

    assert_eq!(
        resolve_history_id_filter(&backend, "TASK-9").unwrap(),
        "TASK-9"
    );
    assert_eq!(
        resolve_history_id_filter(&backend, "task-9").unwrap(),
        "TASK-9"
    );
    assert_eq!(
        resolve_history_id_filter(&backend, &uuid.to_string()).unwrap(),
        "TASK-9"
    );
}

/// A well-formed id that simply isn't live (never added, or since
/// deleted) is NOT refused here — `history::run` is the one that decides,
/// once it knows whether the id has any recorded history at all (a
/// deleted spec can still have a real trail to show).
#[test]
fn resolve_history_id_filter_passes_through_well_formed_unknown_id() {
    let (_dir, backend) = open_backend();
    assert_eq!(
        resolve_history_id_filter(&backend, "TASK-99999").unwrap(),
        "TASK-99999"
    );
}

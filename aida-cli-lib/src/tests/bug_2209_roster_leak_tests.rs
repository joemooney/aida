//! Tests for BUG-2209: a test running as the hermetic test user rewrote the
//! operator's real roster. The seat-grant fixture now refuses any store that
//! is not scratch and merges its one member; the shared roster writers refuse
//! a hermetic-user write outside a temp directory.
// trace:BUG-2209 | ai:claude

use super::test_support::{assert_scratch_store, mint_grant_for};
use super::*;
use crate::test_env::{EnvVarsGuard, HERMETIC_TEST_USER};

fn panics(f: impl FnOnce() + std::panic::UnwindSafe) -> bool {
    std::panic::catch_unwind(f).is_err()
}

/// The fixture writer accepts temp and workspace-`target/` stores and panics
/// for anything else (a real checkout's store).
#[test]
fn scratch_store_guard_accepts_only_temp_and_target() {
    let tmp = tempfile::tempdir().unwrap();
    assert_scratch_store(&tmp.path().join(".aida-store"));
    let target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target");
    std::fs::create_dir_all(&target).unwrap();
    assert_scratch_store(&target);
    assert!(panics(|| assert_scratch_store(Path::new(
        "/nonexistent-bug-2209/.aida-store"
    ))));
    // The checkout itself (where `.aida/config.toml` lives) is not scratch.
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    assert!(panics(|| assert_scratch_store(
        &checkout.join(".aida-store")
    )));
}

/// The shared roster writers (built without `cfg(test)` here) refuse a write
/// made as the hermetic test user outside a temp directory, and only then.
#[test]
fn hermetic_user_roster_write_outside_temp_panics() {
    let real = Path::new("/nonexistent-bug-2209/.aida-store");
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvVarsGuard::apply(&[("AIDA_USER", Some(HERMETIC_TEST_USER))]);
    assert!(panics(|| aida_core::team::guard_roster_write(real)));
    assert!(panics(|| {
        let _ = aida_core::team::allow_seat_cas(real, HERMETIC_TEST_USER, "advisor");
    }));
    aida_core::team::guard_roster_write(&tmp.path().join(".aida-store"));
}

#[test]
fn non_hermetic_user_roster_write_is_not_refused() {
    let _env = EnvVarsGuard::apply(&[("AIDA_USER", Some("bug-2209-operator"))]);
    aida_core::team::guard_roster_write(Path::new("/nonexistent-bug-2209/.aida-store"));
}

/// Minting a seat grant merges the subject into the fixture roster and keeps
/// every other member (it used to rewrite the whole file from a lenient read).
#[test]
fn mint_merges_the_subject_and_keeps_other_members() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida-store/registry")).unwrap();
    std::fs::create_dir_all(root.join(".aida-store/objects")).unwrap();
    let roster = root.join(".aida-store/registry/team.toml");
    std::fs::write(
        &roster,
        "[members]\njoe = [\"advisor\", \"reviewer\"]\n\"joe.mooney\" = [\"advisor\"]\n",
    )
    .unwrap();
    mint_grant_for(root, HERMETIC_TEST_USER, "implementer", &[]);
    mint_grant_for(root, HERMETIC_TEST_USER, "advisor", &["reviewer"]);
    let team = aida_core::team::TeamRoster::load(&root.join(".aida-store"));
    assert_eq!(
        team.seats_for("joe").unwrap(),
        ["advisor", "reviewer"].map(String::from)
    );
    assert_eq!(
        team.seats_for("joe.mooney").unwrap(),
        ["advisor".to_string()]
    );
    assert_eq!(
        team.seats_for(HERMETIC_TEST_USER).unwrap(),
        ["implementer", "advisor", "reviewer"].map(String::from)
    );
}

/// A malformed fixture roster fails the mint instead of being replaced by a
/// one-member roster (the da5ab2b581 shape).
#[test]
fn mint_never_replaces_an_unreadable_roster() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida-store/registry")).unwrap();
    std::fs::create_dir_all(root.join(".aida-store/objects")).unwrap();
    let roster = root.join(".aida-store/registry/team.toml");
    std::fs::write(&roster, "[members]\njoe = [\"advisor\"\n").unwrap();
    let root_buf = root.to_path_buf();
    assert!(panics(move || {
        mint_grant_for(&root_buf, HERMETIC_TEST_USER, "implementer", &[]);
    }));
    assert_eq!(
        std::fs::read_to_string(&roster).unwrap(),
        "[members]\njoe = [\"advisor\"\n"
    );
}

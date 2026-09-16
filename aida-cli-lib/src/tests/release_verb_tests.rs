use super::{preview_next_version, resolve_release_bump};

#[test]
fn resolve_bump_defaults_to_patch() {
    assert_eq!(resolve_release_bump(false, false, false), Ok("patch"));
    assert_eq!(resolve_release_bump(true, false, false), Ok("patch"));
}

#[test]
fn resolve_bump_maps_each_flag() {
    assert_eq!(resolve_release_bump(false, true, false), Ok("minor"));
    assert_eq!(resolve_release_bump(false, false, true), Ok("major"));
}

#[test]
fn resolve_bump_rejects_multiple() {
    assert!(resolve_release_bump(false, true, true).is_err());
    assert!(resolve_release_bump(true, true, false).is_err());
    assert!(resolve_release_bump(true, false, true).is_err());
}

#[test]
fn preview_next_version_bumps_and_resets_lower_components() {
    assert_eq!(
        preview_next_version("0.12.0", "patch").as_deref(),
        Some("0.12.1")
    );
    assert_eq!(
        preview_next_version("0.12.3", "minor").as_deref(),
        Some("0.13.0")
    );
    assert_eq!(
        preview_next_version("0.12.3", "major").as_deref(),
        Some("1.0.0")
    );
}

#[test]
fn preview_next_version_tolerates_prerelease_suffix_and_rejects_garbage() {
    // A -pre / +build suffix is dropped before bumping.
    assert_eq!(
        preview_next_version("1.2.3-rc1", "patch").as_deref(),
        Some("1.2.4")
    );
    // Not a 3-part numeric version → None (handler renders "?").
    assert_eq!(preview_next_version("nightly", "patch"), None);
    assert_eq!(preview_next_version("1.2", "patch"), None);
}

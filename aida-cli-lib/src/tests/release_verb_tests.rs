use super::{preview_next_version, release_artifact_timeout, released_tag, resolve_release_bump};

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

// The artifact wait follows the merged release PR, so it must outlast the
// release workflow plus runner queueing, not the old 10-minute cap.
// trace:TASK-1721 | ai:claude
#[test]
fn release_artifact_timeout_covers_ci_and_honours_override() {
    let default = release_artifact_timeout(None);
    assert!(
        default >= std::time::Duration::from_secs(1800),
        "default artifact wait {default:?} is shorter than CI"
    );
    assert_eq!(
        release_artifact_timeout(Some("2400")),
        std::time::Duration::from_secs(2400)
    );
    assert_eq!(release_artifact_timeout(Some("0")), default);
    assert_eq!(release_artifact_timeout(Some("soon")), default);
}

// release.sh tags the squash-merged commit, which may not be HEAD of the
// checkout, so the wrapper reads the tag release.sh reports.
// trace:TASK-1721 | ai:claude
#[test]
fn released_tag_prefers_the_tag_release_sh_reports() {
    let dir = tempfile::tempdir().unwrap();
    let tag_file = dir.path().join("tag");
    std::fs::write(&tag_file, "v0.15.1\n").unwrap();
    // `dir` is not a git repo, so the describe fallback yields nothing.
    assert_eq!(
        released_tag(&tag_file, dir.path()).as_deref(),
        Some("v0.15.1")
    );
    std::fs::write(&tag_file, "  \n").unwrap();
    assert_eq!(released_tag(&tag_file, dir.path()), None);
}

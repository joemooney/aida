//! `aida node upstream` mirror check fails closed (STORY-095 slice 1 rework).
// trace:STORY-B6B1278890774EF993D2EBC327862470-095 | ai:claude
use super::refuse_mirror_upstream;

#[test]
fn upstream_refused_when_the_project_root_cannot_be_found() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    // No `.git` anywhere above the store: the mirror list is unknown.
    if dir.path().ancestors().any(|p| p.join(".git").exists()) {
        return;
    }
    let err = refuse_mirror_upstream(&store, "gitlab")
        .unwrap_err()
        .to_string();
    assert!(err.contains("refusing to set an upstream"), "{err}");
}

#[test]
fn upstream_refused_when_the_project_config_cannot_be_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join(".aida/config.toml")).unwrap();
    let store = root.join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    // A directory where the config file should be: unreadable as a file.
    let err = refuse_mirror_upstream(&store, "gitlab")
        .unwrap_err()
        .to_string();
    assert!(err.contains("refusing to set an upstream"), "{err}");
}

#[test]
fn upstream_accepted_without_config_and_refused_for_a_mirror() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let store = root.join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    // A missing config means no mirrors are declared.
    refuse_mirror_upstream(&store, "gitlab").unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[store.sync]\nmirror_remotes=['gitlab']\n",
    )
    .unwrap();
    let err = refuse_mirror_upstream(&store, "gitlab")
        .unwrap_err()
        .to_string();
    assert!(err.contains("store mirror"), "{err}");
}

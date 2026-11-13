// trace:TASK-3780 | ai:codex
use super::{shell_substitution_hole_kind, shell_substitution_objects_root};
use std::process::Command;

#[test]
fn known_shell_substitution_holes_are_detected() {
    assert!(shell_substitution_hole_kind("trigger (, RealPhaseDriver)").is_some());
    assert!(shell_substitution_hole_kind("creator's  through forge").is_some());
}

#[test]
fn wrapping_tables_and_ordinary_spacing_are_not_candidates() {
    assert_eq!(shell_substitution_hole_kind("the  next step"), None);
    assert_eq!(shell_substitution_hole_kind("name    value"), None);
    assert_eq!(
        shell_substitution_hole_kind("owner's\n  implementation"),
        None
    );
    assert_eq!(shell_substitution_hole_kind("call `date` safely"), None);
}

#[test]
fn detector_resolves_canonical_store_from_sibling_worktree() {
    fn git(cwd: &std::path::Path, args: &[&str]) {
        let output = crate::test_git::git_command()
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("main");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "--initial-branch=main", "--quiet"]);
    git(&main, &["config", "user.email", "test@example.com"]);
    git(&main, &["config", "user.name", "Test"]);
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    std::fs::write(
        main.join(".aida/config.toml"),
        "[deployment]\nstore_path = \".aida-store\"\n",
    )
    .unwrap();
    git(&main, &["add", ".aida/config.toml"]);
    git(&main, &["commit", "-m", "init", "--quiet"]);
    std::fs::create_dir_all(main.join(".aida-store/objects")).unwrap();

    let sibling = temp.path().join("sibling");
    git(
        &main,
        &[
            "worktree",
            "add",
            "--quiet",
            sibling.to_str().unwrap(),
            "-b",
            "feature",
        ],
    );
    assert_eq!(
        shell_substitution_objects_root(&sibling)
            .canonicalize()
            .unwrap(),
        main.join(".aida-store/objects").canonicalize().unwrap()
    );
    git(
        &main,
        &["worktree", "remove", "--force", sibling.to_str().unwrap()],
    );
}

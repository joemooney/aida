//! README ownership across managed diffs, upgrade and refresh.
// trace:BUG-A65B4BF5902D469995F816EFD3A27661-001 | ai:codex

use super::*;
use aida_core::scaffolding::{ScaffoldArtifact, ScaffoldPreview};
use std::path::{Path, PathBuf};

fn preview() -> ScaffoldPreview {
    ScaffoldPreview {
        artifacts: [
            ("README.md", "# Starter project\n"),
            (".claude/commands/example.md", "Current managed command\n"),
        ]
        .into_iter()
        .map(|(path, content)| ScaffoldArtifact {
            path: PathBuf::from(path),
            content: content.into(),
            description: "seed ownership fixture".into(),
            exists: false,
            file_status: aida_core::scaffolding::FileStatus::New,
        })
        .collect(),
        overwrites: vec![],
        new_files: vec![],
        new_dirs: vec![],
        modified_files: vec![],
        upgradeable_files: vec![],
        skill_packs: vec![],
    }
}

#[test]
fn readme_seed_diff_preserves_managed_and_missing_exit_semantics() {
    let root = tempfile::tempdir().unwrap();
    let preview = preview();
    let all: Vec<_> = preview.artifacts.iter().collect();
    let readme = root.path().join("README.md");
    let command = root.path().join(".claude/commands/example.md");
    std::fs::create_dir_all(command.parent().unwrap()).unwrap();
    std::fs::write(&command, &preview.artifacts[1].content).unwrap();
    std::fs::write(
        &readme,
        "# Authored product\n\n## Usage\n\nA completely different guide.\n",
    )
    .unwrap();
    assert!(file_matches_artifact(
        Path::new("README.md"),
        "authored",
        "starter"
    ));
    // The returned drift flag directly controls the command's 0/1 exit code.
    for list_only in [false, true] {
        assert!(!print_scaffold_diffs(root.path(), &all, 3, true, list_only).unwrap());
        // An explicit README path does not re-enable a starter comparison.
        assert!(!print_scaffold_diffs(root.path(), &all[..1], 3, true, list_only).unwrap());
        std::fs::remove_file(&readme).unwrap();
        assert!(print_scaffold_diffs(root.path(), &all, 3, true, list_only).unwrap());
        assert!(print_scaffold_diffs(root.path(), &all[..1], 3, true, list_only).unwrap());
        std::fs::write(&readme, "authored").unwrap();
    }
    std::fs::write(&readme, [b'#', b' ', 0xe9, 0xff]).unwrap();
    for list_only in [false, true] {
        assert!(!print_scaffold_diffs(root.path(), &all, 3, true, list_only).unwrap());
        assert!(!print_scaffold_diffs(root.path(), &all[..1], 3, true, list_only).unwrap());
    }
    std::fs::write(&command, "stale managed command").unwrap();
    assert!(print_scaffold_diffs(root.path(), &all, 3, true, true).unwrap());
    assert!(!file_matches_artifact(
        &preview.artifacts[1].path,
        "stale",
        &preview.artifacts[1].content
    ));
}

#[test]
fn readme_seed_upgrade_creates_missing_and_preserves_authored_content() {
    let root = tempfile::tempdir().unwrap();
    let preview = preview();
    let readme = root.path().join("README.md");
    run_scaffold_upgrade(root.path(), &preview, true, false, false).unwrap();
    assert!(!readme.exists(), "dry run must not create a seed");
    run_scaffold_upgrade(root.path(), &preview, false, false, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(&readme).unwrap(),
        preview.artifacts[0].content
    );

    let authored = format!(
        "# Example product\n\n{}",
        "## Usage\n\nAuthored instructions.\n\n".repeat(40)
    );
    std::fs::write(&readme, &authored).unwrap();
    let command = root.path().join(&preview.artifacts[1].path);
    std::fs::write(&command, "stale managed command").unwrap();
    run_scaffold_upgrade(root.path(), &preview, false, false, false).unwrap();
    assert_eq!(std::fs::read_to_string(&readme).unwrap(), authored);
    assert_eq!(
        std::fs::read_to_string(&command).unwrap(),
        preview.artifacts[1].content
    );
    assert!(!print_scaffold_diffs(
        root.path(),
        &preview.artifacts.iter().collect::<Vec<_>>(),
        3,
        true,
        true
    )
    .unwrap());

    let encoded = [b'#', b' ', 0xe9, 0xff];
    std::fs::write(&readme, encoded).unwrap();
    run_scaffold_upgrade(root.path(), &preview, false, false, false).unwrap();
    assert_eq!(std::fs::read(&readme).unwrap(), encoded);
    run_scaffold_upgrade(root.path(), &preview, false, true, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(&readme).unwrap(),
        preview.artifacts[0].content
    );
}

#[test]
fn readme_seed_refresh_leaves_existing_and_missing_seed_alone() {
    let root = tempfile::tempdir().unwrap();
    let prompts = tempfile::tempdir().unwrap();
    let readme = root.path().join("README.md");
    std::fs::write(&readme, "# Authored product\n\nKeep these instructions.\n").unwrap();
    crate::scaffold_refresh::refresh_agent_packs_at(root.path(), Some(prompts.path()), None)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&readme).unwrap(),
        "# Authored product\n\nKeep these instructions.\n"
    );
    let encoded = [b'#', b' ', 0xe9, 0xff];
    std::fs::write(&readme, encoded).unwrap();
    crate::scaffold_refresh::refresh_agent_packs_at(root.path(), Some(prompts.path()), None)
        .unwrap();
    assert_eq!(std::fs::read(&readme).unwrap(), encoded);
    std::fs::remove_file(&readme).unwrap();
    crate::scaffold_refresh::refresh_agent_packs_at(root.path(), Some(prompts.path()), None)
        .unwrap();
    assert!(
        !readme.exists(),
        "pack refresh does not create project seeds"
    );
}

#[test]
fn project_registry_is_created_with_current_project() {
    let temp = tempfile::tempdir().unwrap();
    let registry = temp.path().join("home/.aida/projects.toml");
    let root = temp.path().join("demo");
    std::fs::create_dir_all(&root).unwrap();

    assert!(crate::register_project_in_registry_at(&registry, &root).unwrap());
    let body = std::fs::read_to_string(&registry).unwrap();

    assert!(body.starts_with("# AIDA project registry\n"));
    assert!(body.contains("[[project]]"));
    assert!(body.contains("name = \"demo\""));
    assert!(body.contains(&format!("path = \"{}\"", root.display())));

    assert!(!crate::register_project_in_registry_at(&registry, &root).unwrap());
}

#[test]
fn project_registry_updates_path_match_and_preserves_other_blocks() {
    let temp = tempfile::tempdir().unwrap();
    let registry = temp.path().join("projects.toml");
    let root = temp.path().join("demo");
    std::fs::create_dir_all(&root).unwrap();

    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["remote", "add", "origin", "git@example.test:team/demo.git"])
        .current_dir(&root)
        .status()
        .unwrap();

    let original = format!(
        "\
# header stays

[[project]]
# row comment stays
name = \"old-demo\"
path = \"{}\"
repo = \"old\"

[[relation]]
from = \"one\"
to = \"two\"
",
        root.display()
    );
    std::fs::write(&registry, &original).unwrap();

    assert!(crate::register_project_in_registry_at(&registry, &root).unwrap());
    let body = std::fs::read_to_string(&registry).unwrap();

    assert!(body.contains("# header stays"));
    assert!(body.contains("# row comment stays"));
    assert!(body.contains("[[relation]]\nfrom = \"one\"\nto = \"two\""));
    assert_eq!(body.matches("[[project]]").count(), 1);
    assert!(body.contains("name = \"demo\""));
    assert!(body.contains("repo = \"git@example.test:team/demo.git\""));
}

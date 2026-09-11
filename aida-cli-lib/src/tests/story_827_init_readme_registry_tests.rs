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
    let canonical_root = root.canonicalize().unwrap();
    // trace:BUG-1021 | ai:claude
    // toml_edit emits a literal `'...'` string for a backslash-bearing Windows
    // path, so compare the parsed value rather than an escaped substring.
    let doc: toml::Value = body.parse().unwrap();
    let recorded = doc["project"][0]["path"].as_str().unwrap();
    assert_eq!(recorded, canonical_root.display().to_string());

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

    let canonical_root = root.canonicalize().unwrap();
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
        canonical_root.display().to_string().replace('\\', "\\\\")
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

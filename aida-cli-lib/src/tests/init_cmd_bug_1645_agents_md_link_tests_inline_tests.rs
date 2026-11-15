use super::complete_init_scaffolding;

/// Run the full init scaffold (Codex agent, so AGENTS.md is written)
/// into `root`, where AGENTS.md is a symlink to `target`.
fn init_with_agents_link(root: &std::path::Path, target: &std::path::Path) {
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(root.join(".gitignore"), ".aida/*\n!.aida/config.toml\n").unwrap();
    std::os::unix::fs::symlink(target, root.join("AGENTS.md")).unwrap();
    let store = aida_core::models::RequirementsStore::new();
    complete_init_scaffolding(
        root,
        &store,
        Some("codex"),
        false, // with_mcp
        true,  // no_skills
        true,  // no_hooks
        false, // force
        root.join(".aida-store"),
        "test store",
        false, // verbose
        crate::cli::InitFootprint::Full,
        true, // suppress scaffold commit
    )
    .unwrap();
}

#[test]
fn bug_1645_agents_md_link_into_project_gets_the_aida_block() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("CLAUDE.md"), "# My project\n").unwrap();
    init_with_agents_link(root, std::path::Path::new("CLAUDE.md"));
    assert!(root
        .join("AGENTS.md")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    let claude = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
    assert!(claude.starts_with("# My project"), "{claude}");
    assert!(claude.contains("<!-- AIDA-AUTOGEN-BEGIN -->"), "{claude}");
}

#[test]
fn bug_1645_agents_md_link_outside_project_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("SHARED.md");
    std::fs::write(&target, "shared\n").unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    init_with_agents_link(&root, &target);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "shared\n");
    assert!(!super::agents_md_link_merge_allowed(
        &root,
        &root.join("AGENTS.md")
    ));
}

#[test]
fn bug_1645_agents_md_link_to_template_master_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let master = root.join("aida-core/templates/AGENTS.md");
    std::fs::create_dir_all(master.parent().unwrap()).unwrap();
    std::fs::write(&master, "master\n").unwrap();
    init_with_agents_link(root, &master);
    assert_eq!(std::fs::read_to_string(&master).unwrap(), "master\n");
    assert!(!super::agents_md_link_merge_allowed(
        root,
        &root.join("AGENTS.md")
    ));
}

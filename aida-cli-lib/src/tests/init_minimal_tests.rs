use super::{
    install_user_aida_instructions_at, install_user_aida_instructions_if_accepted,
    merge_user_aida_instructions, render_user_aida_instructions_block, resolve_spec_from_markdown,
    scaffold_minimal_specs, USER_AIDA_INSTRUCTIONS_BEGIN,
};

#[test]
fn minimal_scaffold_is_a_bare_folder_that_the_magic_can_read() {
    let dir = tempfile::tempdir().unwrap();
    let created = scaffold_minimal_specs(dir.path(), false).unwrap();
    assert_eq!(created.len(), 2, "creates the spec + the demo file");

    // Only markdown + a code file — no machine.
    assert!(dir.path().join("specs/EXAMPLE-1.md").exists());
    assert!(dir.path().join("example.py").exists());
    assert!(
        !dir.path().join(".aida").exists(),
        "no orphan-branch machine"
    );
    assert!(!dir.path().join(".aida-store").exists());

    // The whole point: `aida why` resolves the scaffolded id from the bare
    // folder — init --minimal and the magic compose with zero setup.
    let it = resolve_spec_from_markdown(dir.path(), "EXAMPLE-1").expect("resolves the demo spec");
    assert_eq!(it.title, "Rate-limit the login endpoint");
    assert!(it.markdown.is_some());

    // Idempotent guard: a second run without --force writes nothing.
    assert!(scaffold_minimal_specs(dir.path(), false)
        .unwrap()
        .is_empty());
}

#[test]
fn declined_user_aida_instructions_offer_leaves_user_files_untouched() {
    let home = tempfile::tempdir().unwrap();

    let installed = install_user_aida_instructions_if_accepted(home.path(), false).unwrap();

    assert!(installed.is_none());
    assert!(!home.path().join(".claude/CLAUDE.md").exists());
    assert!(!home.path().join(".codex/AGENTS.md").exists());
}

#[test]
fn user_aida_instructions_install_preserves_existing_user_content() {
    let home = tempfile::tempdir().unwrap();
    let claude = home.path().join(".claude/CLAUDE.md");
    std::fs::create_dir_all(claude.parent().unwrap()).unwrap();
    std::fs::write(&claude, "personal rule\n").unwrap();

    let report = install_user_aida_instructions_at(home.path(), false).unwrap();

    assert_eq!(report.written, 2);
    let claude_body = std::fs::read_to_string(&claude).unwrap();
    assert!(claude_body.starts_with("personal rule\n\n"));
    assert!(claude_body.contains(USER_AIDA_INSTRUCTIONS_BEGIN));
    assert!(
        std::fs::read_to_string(home.path().join(".codex/AGENTS.md"))
            .unwrap()
            .contains(USER_AIDA_INSTRUCTIONS_BEGIN)
    );

    let rerun = install_user_aida_instructions_at(home.path(), false).unwrap();
    assert_eq!(rerun.unchanged, 2);
}

#[test]
fn user_aida_instructions_refresh_keeps_edited_blocks() {
    let block = render_user_aida_instructions_block();
    let edited = block.replace(
        "requirements live in AIDA",
        "requirements live somewhere else",
    );
    let existing = format!("top\n\n{edited}\nbottom\n");

    let (merged, report) = merge_user_aida_instructions(Some(&existing), true);

    assert_eq!(merged, existing);
    assert_eq!(report.kept_edited, 1);
}

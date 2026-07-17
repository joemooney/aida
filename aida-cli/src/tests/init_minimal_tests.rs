use super::{resolve_spec_from_markdown, scaffold_minimal_specs};

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

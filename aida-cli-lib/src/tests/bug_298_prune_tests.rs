use super::*;

/// BUG-298: only obsolete `aida-*` files are flagged — a currently-shipped
/// skill, a user's own non-`aida-` file, and any symlink are all left alone.
#[test]
fn detect_obe_aida_scaffold_files_flags_only_obsolete() {
    let dir = tempfile::tempdir().unwrap();
    let skills = dir.path().join(".claude/skills");
    std::fs::create_dir_all(&skills).unwrap();
    std::fs::write(skills.join("aida-req.md"), "x").unwrap(); // shipped → keep
    std::fs::write(skills.join("aida-obsolete-xyz.md"), "x").unwrap(); // OBE → flag
    std::fs::write(skills.join("my-skill.md"), "x").unwrap(); // user → keep
    #[cfg(unix)]
    std::os::unix::fs::symlink("/nonexistent", skills.join("aida-symlinked.md")).unwrap();

    let names: Vec<String> = detect_obe_aida_scaffold_files(dir.path())
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
        .collect();
    assert!(
        names.contains(&"aida-obsolete-xyz.md".to_string()),
        "obsolete aida file must be flagged: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "aida-req.md"),
        "a shipped skill must NOT be flagged: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "my-skill.md"),
        "a user's non-aida file must NOT be flagged: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "aida-symlinked.md"),
        "a symlink must NOT be flagged: {names:?}"
    );
}

/// BUG-719: the shared OBE handler removes an obsolete/resurrected `aida-*`
/// file (e.g. `aida-recover`, deleted upstream by TASK-584 but re-created by
/// an older binary whose embedded set still has it) when `prune` is set, and
/// leaves a shipped skill + a user's own file untouched.
#[test]
fn report_and_prune_obe_scaffold_removes_resurrected_aida_recover() {
    let dir = tempfile::tempdir().unwrap();
    let skills = dir.path().join(".claude/skills");
    std::fs::create_dir_all(&skills).unwrap();
    let recover = skills.join("aida-recover.md"); // removed upstream → OBE
    let shipped = skills.join("aida-req.md"); // shipped → keep
    let user = skills.join("my-skill.md"); // user → keep
    std::fs::write(&recover, "<!-- AIDA Generated -->\n").unwrap();
    std::fs::write(&shipped, "x").unwrap();
    std::fs::write(&user, "x").unwrap();

    // Report-only (prune=false) must not remove anything.
    report_and_prune_obe_scaffold(dir.path(), false, false, "hint");
    assert!(recover.exists(), "report-only must not remove the OBE file");

    // Prune removes ONLY the obsolete aida-* file.
    report_and_prune_obe_scaffold(dir.path(), true, false, "hint");
    assert!(
        !recover.exists(),
        "prune must remove the resurrected aida-recover (BUG-719)"
    );
    assert!(shipped.exists(), "a shipped skill must survive prune");
    assert!(user.exists(), "a user's own file must survive prune");
}

/// BUG-719: `--prune` combined with `--dry-run` reports but removes nothing.
#[test]
fn report_and_prune_obe_scaffold_dry_run_keeps_file() {
    let dir = tempfile::tempdir().unwrap();
    let skills = dir.path().join(".claude/skills");
    std::fs::create_dir_all(&skills).unwrap();
    let recover = skills.join("aida-recover.md");
    std::fs::write(&recover, "x").unwrap();
    report_and_prune_obe_scaffold(dir.path(), true, true, "hint");
    assert!(
        recover.exists(),
        "dry-run must not remove even with --prune"
    );
}

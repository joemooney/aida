use super::*;
use tempfile::TempDir;

/// Fresh project (no .gitignore) → file is created with both blocks.
/// Returns Ok(false) per the contract (a brand-new file isn't an
// "update"). trace:BUG-73 | ai:claude
#[test]
fn creates_gitignore_with_both_blocks() {
    let tmp = TempDir::new().unwrap();
    let updated = add_aida_gitignore_entries(tmp.path(), ".aida-store").unwrap();
    assert!(!updated, "creation isn't an update");
    let content = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(content.contains(".aida-store/"), "{}", content);
    assert!(has_aida_runtime_deny_pattern(&content), "{}", content);
    assert!(content.contains("!.aida/config.toml"), "{}", content);
    // trace:BUG-484 — the per-user MCP-trust file must be gitignored so a
    // committed pre-approval can't become a clone-attack vector.
    assert!(
        content.contains(".claude/settings.local.json"),
        "{}",
        content
    );
}

/// Existing .gitignore that already covers ALL six blocks (store,
/// runtime, CLAUDE.local.md, rules/aida-specs/, docs/plans/_draft/,
/// settings.local.json) → no write, returns Ok(false). The third block
/// (CLAUDE.local.md) was added in commit bf50e7c0 for TASK-572. The fourth
/// (.claude/rules/aida-specs/) was added by SPIKE-31. The fifth
/// (docs/plans/_draft/) by TASK-383. The sixth (settings.local.json) by
// BUG-484.
// trace:BUG-73 trace:TASK-572 trace:SPIKE-31 trace:TASK-383 trace:BUG-484 | ai:claude
#[test]
fn idempotent_when_both_blocks_present() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".gitignore");
    let original = "target/\n.aida-store/\n.aida/*\n!.aida/config.toml\nCLAUDE.local.md\n.claude/rules/aida-specs/\ndocs/plans/_draft/\n.claude/settings.local.json\n";
    std::fs::write(&path, original).unwrap();
    let updated = add_aida_gitignore_entries(tmp.path(), ".aida-store").unwrap();
    assert!(
        !updated,
        "all six blocks already present → expected no append"
    );
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, original);
}

/// .gitignore lacking only the settings.local.json block → that block is
// appended (and only that one). trace:BUG-484 | ai:claude
#[test]
fn appends_settings_local_when_missing() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".gitignore");
    let original = "target/\n.aida-store/\n.aida-store\n.aida/*\n!.aida/config.toml\nCLAUDE.local.md\n.claude/rules/aida-specs/\ndocs/plans/_draft/\n";
    std::fs::write(&path, original).unwrap();
    let updated = add_aida_gitignore_entries(tmp.path(), ".aida-store").unwrap();
    assert!(
        updated,
        "settings.local.json block missing → expected append"
    );
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains(".claude/settings.local.json"));
    // Idempotent on a second pass.
    let updated2 = add_aida_gitignore_entries(tmp.path(), ".aida-store").unwrap();
    assert!(!updated2, "second pass should be a no-op");
}

/// Legacy .gitignore (has `.aida-store/` but pre-BUG-73 per-file ignores)
/// → the deny block is appended; the old per-file lines are left in place
/// (harmless but redundant). Migration path for existing projects.
// trace:BUG-73 | ai:claude
#[test]
fn appends_deny_block_when_only_legacy_per_file_entries() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".gitignore");
    std::fs::write(
        &path,
        "target/\n.aida-store/\n.aida/session-env.sh\n.aida/sessions/\n",
    )
    .unwrap();
    let updated = add_aida_gitignore_entries(tmp.path(), ".aida-store").unwrap();
    assert!(updated, "deny block missing → expected an append");
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(has_aida_runtime_deny_pattern(&content));
    assert!(content.contains("!.aida/config.toml"));
    // Legacy entries preserved
    assert!(content.starts_with("target/\n.aida-store/\n.aida/session-env.sh\n"));
}

/// .gitignore exists but no AIDA blocks → both get appended.
// trace:BUG-73 | ai:claude
#[test]
fn appends_both_when_neither_present() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".gitignore");
    std::fs::write(&path, "target/\n").unwrap();
    let updated = add_aida_gitignore_entries(tmp.path(), ".aida-store").unwrap();
    assert!(updated);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains(".aida-store/"));
    assert!(has_aida_runtime_deny_pattern(&content));
    assert!(content.contains("!.aida/config.toml"));
}

/// Detection helper: matches a bare `.aida/*` line, ignores comments
/// that mention the string, ignores leading/trailing whitespace.
// trace:BUG-73 | ai:claude
#[test]
fn deny_pattern_detection() {
    assert!(has_aida_runtime_deny_pattern(".aida/*\n"));
    assert!(has_aida_runtime_deny_pattern("foo\n  .aida/*  \nbar\n"));
    assert!(!has_aida_runtime_deny_pattern("# .aida/* is a comment\n"));
    assert!(!has_aida_runtime_deny_pattern(".aida/session-env.sh\n"));
    assert!(!has_aida_runtime_deny_pattern(""));
}

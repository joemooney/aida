use super::{
    init_scaffold_commit_paths, portable_pack_commit_paths, write_memory_lane_scaffolding,
};
use aida_core::scaffolding::refresh::read_skill_manifest;
use aida_core::scaffolding::refresh::DELIVERED_MANIFEST;

fn full_install(root: &std::path::Path) {
    let mut scaffolder = aida_core::scaffolding::Scaffolder::new(
        root.to_path_buf(),
        aida_core::scaffolding::ScaffoldConfig::default(),
    );
    let preview = scaffolder.preview(&aida_core::RequirementsStore::default());
    scaffolder.apply(&preview).unwrap();
}

// trace:TASK-1528 | ai:codex
#[test]
fn claude_only_memory_lane_does_not_write_portable_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = aida_core::RequirementsStore::default();
    super::write_enabled_agent_selection(
        root,
        super::AgentSelection {
            claude: true,
            codex: false,
            antigravity: false,
        },
    )
    .unwrap();
    write_memory_lane_scaffolding(root, &store, "test", false, false).unwrap();
    assert!(!root.join(".agents/skills").exists());
    assert!(root.join(".claude/skills/aida-capture/SKILL.md").is_file());
}

/// BUG-1645 L4: memory-lane init's scaffold commit includes the per-pack
/// delivered-skills manifests, so they are not left uncommitted.
// trace:BUG-1645 | ai:claude
#[test]
fn bug_1645_memory_lane_commit_paths_include_manifests() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = aida_core::RequirementsStore::default();
    write_memory_lane_scaffolding(root, &store, "test", false, false).unwrap();
    let paths = init_scaffold_commit_paths(root, crate::cli::InitFootprint::MemoryLane);
    for manifest in [
        ".claude/skills/.aida-delivered",
        ".agents/skills/.aida-delivered", // trace:BUG-1639 | ai:claude
    ] {
        assert!(root.join(manifest).is_file(), "{manifest} written");
        assert!(paths.iter().any(|p| p == manifest), "{manifest}: {paths:?}");
    }
}

/// A full init's commit stages only the portable skills AIDA ships and
/// its manifest, never another tool's skill or a user-authored `aida-*`
/// skill in the shared `.agents/skills`.
// trace:BUG-1639 | ai:claude
#[test]
fn portable_pack_commit_paths_stage_only_shipped_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let pack = root.join(".agents/skills");
    for name in ["aida-handoff", "aida-my-own", "typesafe-ai"] {
        std::fs::create_dir_all(pack.join(name)).unwrap();
        std::fs::write(pack.join(name).join("SKILL.md"), "x\n").unwrap();
    }
    std::fs::write(pack.join(DELIVERED_MANIFEST), "x\n").unwrap();
    assert_eq!(
        portable_pack_commit_paths(root),
        [
            ".agents/skills/.aida-delivered",
            ".agents/skills/aida-handoff"
        ]
    );
}

/// BUG-1645 review: memory-lane never writes through a symlinked skill
/// pack or a symlinked SKILL.md, even under --force.
// trace:BUG-1645 | ai:claude
#[cfg(unix)]
#[test]
fn bug_1645_memory_lane_refuses_symlinks_even_with_force() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = aida_core::RequirementsStore::default();
    // A symlinked .codex/skills pack with nothing in it.
    let shared = root.join("shared-codex");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::create_dir_all(root.join(".codex")).unwrap();
    std::os::unix::fs::symlink(&shared, root.join(".codex/skills")).unwrap();
    // A symlinked shared .agents/skills pack. trace:BUG-1639 | ai:claude
    std::fs::create_dir_all(root.join(".agents")).unwrap();
    std::os::unix::fs::symlink(&shared, root.join(".agents/skills")).unwrap();
    // A per-file symlinked .claude SKILL.md.
    let mine = root.join("my-capture.md");
    std::fs::write(&mine, "mine\n").unwrap();
    std::fs::create_dir_all(root.join(".claude/skills/aida-capture")).unwrap();
    std::os::unix::fs::symlink(&mine, root.join(".claude/skills/aida-capture/SKILL.md")).unwrap();

    for force in [false, true] {
        write_memory_lane_scaffolding(root, &store, "test", false, force).unwrap();
        assert_eq!(
            std::fs::read_dir(&shared).unwrap().count(),
            0,
            "force={force}"
        );
        assert_eq!(
            std::fs::read_to_string(&mine).unwrap(),
            "mine\n",
            "force={force}"
        );
    }
    assert!(root.join(".claude/skills/aida-learn/SKILL.md").is_file());
}

/// Memory-lane records its skills in each pack's manifest, a re-run never
/// resurrects a deleted one, and a later full `aida init` delivers the
/// full skill set (not ~55 permanent opt-outs).
// trace:TASK-1503 | ai:claude
#[test]
fn memory_lane_records_manifest_and_full_init_delivers_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = aida_core::RequirementsStore::default();
    write_memory_lane_scaffolding(root, &store, "test", false, false).unwrap();
    for pack in [".claude/skills", ".agents/skills"] {
        let m = read_skill_manifest(&root.join(pack)).unwrap().unwrap();
        assert!(!m.complete, "{pack}: two skills never complete a manifest");
        assert!(m.delivered.contains("aida-capture") && m.delivered.contains("aida-learn"));
    }

    std::fs::remove_dir_all(root.join(".claude/skills/aida-capture")).unwrap();
    write_memory_lane_scaffolding(root, &store, "test", false, true).unwrap();
    assert!(
        !root.join(".claude/skills/aida-capture").exists(),
        "memory-lane re-run (even --force) keeps the deletion"
    );

    full_install(root);
    for pack in [".claude/skills", ".agents/skills"] {
        let dir = root.join(pack);
        for name in ["aida-req", "aida-commit", "aida-orchestrate", "aida-learn"] {
            assert!(dir.join(name).join("SKILL.md").is_file(), "{pack}/{name}");
        }
        let m = read_skill_manifest(&dir).unwrap().unwrap();
        assert!(m.complete, "{pack}: the full install completes it");
        assert!(m.unconfirmed.is_empty(), "{pack}: {m:?}");
    }
    assert!(!root.join(".claude/skills/aida-capture").exists());
    assert!(root.join(".agents/skills/aida-capture/SKILL.md").is_file());
    let m = read_skill_manifest(&root.join(".claude/skills"))
        .unwrap()
        .unwrap();
    assert_eq!(
        m.opted_out.iter().collect::<Vec<_>>(),
        ["aida-capture"],
        "only the real deletion is an opt-out"
    );
}

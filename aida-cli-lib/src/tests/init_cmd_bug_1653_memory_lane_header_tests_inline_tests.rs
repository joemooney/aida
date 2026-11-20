use super::{write_memory_lane_scaffolding, MEMORY_LANE_SKILLS};

/// Memory-lane writes each skill byte-for-byte as the full scaffold would
/// (AIDA-Generated header included), in both packs.
// trace:BUG-1653 | ai:claude
#[test]
fn bug_1653_memory_lane_skills_match_full_scaffold_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = aida_core::RequirementsStore::default();
    write_memory_lane_scaffolding(root, &store, "test", false, false).unwrap();

    let empty = tempfile::tempdir().unwrap();
    let preview = aida_core::scaffolding::Scaffolder::new(
        empty.path().to_path_buf(),
        aida_core::scaffolding::ScaffoldConfig::default(),
    )
    .preview(&store);
    for pack in [".claude/skills", ".agents/skills"] {
        for name in MEMORY_LANE_SKILLS {
            let rel = std::path::PathBuf::from(format!("{pack}/{name}/SKILL.md"));
            let expected = &preview
                .artifacts
                .iter()
                .find(|a| a.path == rel)
                .unwrap_or_else(|| panic!("full scaffold renders {}", rel.display()))
                .content;
            let on_disk = std::fs::read_to_string(root.join(&rel)).unwrap();
            assert!(
                on_disk.contains("<!-- AIDA Generated: v"),
                "{}",
                rel.display()
            );
            assert_eq!(&on_disk, expected, "{}", rel.display());
        }
    }
}

use std::path::PathBuf;

use aida_core::scaffolding::{ScaffoldConfig, Scaffolder};
use aida_core::RequirementsStore;

fn budgets() -> toml::Value {
    toml::from_str(include_str!("../templates/context-budget.toml")).unwrap()
}

fn limit(name: &str) -> usize {
    budgets()[name].as_integer().unwrap() as usize
}

fn frontmatter_description_bytes(body: &str) -> usize {
    body.lines()
        .find_map(|line| line.strip_prefix("description:"))
        .map(|value| value.trim().len())
        .unwrap_or(0)
}

#[test]
fn repository_and_generated_scaffold_stay_inside_context_budgets() {
    // trace:TASK-1441 | ai:codex
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = crate_root.parent().unwrap();
    let repo_claude = std::fs::read(repo_root.join("CLAUDE.md")).unwrap();
    assert!(
        repo_claude.len() <= limit("repo_claude_md_max_bytes"),
        "CLAUDE.md is {} bytes; budget is {}",
        repo_claude.len(),
        limit("repo_claude_md_max_bytes")
    );

    let scratch = tempfile::tempdir().unwrap();
    let mut scaffolder = Scaffolder::new(scratch.path().to_path_buf(), ScaffoldConfig::default());
    let store = RequirementsStore {
        name: "context-budget-probe".into(),
        title: "Context Budget Probe".into(),
        ..Default::default()
    };
    let preview = scaffolder.preview(&store);
    let mut baseline = 0usize;
    let mut skill_names = std::collections::HashSet::new();
    let mut command_names = std::collections::HashSet::new();
    for artifact in &preview.artifacts {
        let path = artifact.path.to_string_lossy();
        if path == "CLAUDE.md" || path == ".claude/AIDA.md" || path == ".aida/discipline/README.md"
        {
            baseline += artifact.content.len();
        }
        if path.starts_with(".claude/skills/") && path.ends_with("SKILL.md") {
            let bytes = frontmatter_description_bytes(&artifact.content);
            assert!(
                bytes <= limit("skill_description_max_bytes"),
                "{path}: {bytes} bytes"
            );
            baseline += bytes;
            if let Some(name) = artifact.path.parent().and_then(|p| p.file_name()) {
                skill_names.insert(name.to_string_lossy().into_owned());
            }
        }
        if path.starts_with(".claude/commands/") && path.ends_with(".md") {
            let bytes = frontmatter_description_bytes(&artifact.content);
            assert!(
                bytes <= limit("command_description_max_bytes"),
                "{path}: {bytes} bytes"
            );
            baseline += bytes;
            if let Some(name) = artifact.path.file_stem() {
                command_names.insert(name.to_string_lossy().into_owned());
            }
        }
    }
    for hidden in ["aida-advise", "aida-assess", "aida-intent"] {
        assert!(
            !skill_names.contains(hidden),
            "headless-only skill {hidden} leaked"
        );
        assert!(
            !command_names.contains(hidden),
            "headless-only command {hidden} leaked"
        );
    }
    assert!(
        baseline <= limit("scaffold_baseline_max_bytes"),
        "generated always-loaded scaffold is {baseline} bytes; budget is {}",
        limit("scaffold_baseline_max_bytes")
    );
    eprintln!(
        "context-budget: repo_claude={} scaffold_baseline={} skill_names={} command_names={}",
        repo_claude.len(),
        baseline,
        skill_names.len(),
        command_names.len()
    );
}

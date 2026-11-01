//! `aida skill` — render a skill with its local overlay, and lint skills.
// trace:STORY-1728 | ai:claude

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Stock and local skill tooling.
#[derive(clap::Parser, Debug)]
#[clap(name = "skill")]
pub(crate) struct SkillArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::SkillCommand,
}

// Handle skill commands before storage resolution — needs no DB.
fn run_skill(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    use crate::*;
    let args = <SkillArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let skill_cmd = &args.inner;
    match skill_cmd {
        SkillCommand::Render { name } => {
            let project_root = find_project_root()
                .or_else(|_| std::env::current_dir())
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Error: could not find project root or current directory: {}",
                        e
                    )
                })?;
            print!("{}", render_skill(&project_root, name)?);
            return Ok(());
        }
        // `aida skill lint` reads discovered skills and the plan files
        // they reference — purely filesystem, no store handle. Dispatch it
        // here alongside the other skill tooling. trace:TASK-927 | ai:claude
        SkillCommand::Lint { skill, json, quiet } => {
            return lint_skills(skill.as_deref(), *json, *quiet);
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static SKILL: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "skill",
        help_after: "commands",
        command: <SkillArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_skill),
    };

/// Discover prompt files, excluding overlays and folder helper Markdown.
/// Pack order also defines render precedence; folder form precedes legacy flat.
// trace:BUG-2388 | ai:codex
pub(crate) fn discover_skill_files(project_root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut found_pack = false;
    for pack in [".agents/skills", ".claude/skills"] {
        let dir = project_root.join(pack);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("could not read {}", dir.display())),
        };
        found_pack = true;
        let mut folders = Vec::new();
        let mut flat = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.is_dir() {
                let prompt = path.join("SKILL.md");
                if prompt.is_file() {
                    folders.push(prompt);
                }
            } else if pack == ".claude/skills" && path.is_file() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.ends_with(".md") && !name.ends_with(".local.md") {
                    flat.push(path);
                }
            }
        }
        folders.sort();
        flat.sort();
        files.extend(folders);
        files.extend(flat);
    }
    if !found_pack {
        anyhow::bail!("no skills directory at .agents/skills/ or .claude/skills/ — run `aida init` to scaffold skills");
    }
    Ok(files)
}

// trace:BUG-2388 | ai:codex
fn is_folder_prompt(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == "SKILL.md")
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|n| n == "skills")
}

/// Render the first matching prompt with its local overlay. Folder skills use
/// SKILL.local.md, falling back to the pack's historical <name>.local.md.
// trace:BUG-2388 | ai:codex
pub(crate) fn render_skill(project_root: &Path, name: &str) -> Result<String> {
    let files = discover_skill_files(project_root)?;
    let stock_path = files
        .into_iter()
        .find(|path| {
            if is_folder_prompt(path) {
                path.parent()
                    .and_then(Path::file_name)
                    .is_some_and(|n| n == name)
            } else {
                path.file_stem().is_some_and(|n| n == name)
            }
        })
        .with_context(|| {
            format!("skill '{name}' not found under .agents/skills/ or .claude/skills/")
        })?;
    let mut content = std::fs::read_to_string(&stock_path)
        .with_context(|| format!("failed to read skill {}", stock_path.display()))?;
    let parent = stock_path.parent().expect("discovered skill has a parent");
    let folder_form = is_folder_prompt(&stock_path);
    let local_path = if folder_form && parent.join("SKILL.local.md").exists() {
        parent.join("SKILL.local.md")
    } else {
        let pack = if folder_form {
            parent.parent().expect("skill pack")
        } else {
            parent
        };
        pack.join(format!("{name}.local.md"))
    };
    if local_path.exists() {
        let local = std::fs::read_to_string(&local_path)
            .with_context(|| format!("failed to read local skill {}", local_path.display()))?;
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&local);
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, body: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    // trace:BUG-2388 | ai:codex
    #[test]
    fn skill_discovery_codex_only_render_and_lint() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, ".agents/skills/pickup/SKILL.md", "portable");
        write(root, ".agents/skills/pickup/SKILL.local.md", "local\n");
        write(root, ".agents/skills/pickup/examples/helper.md", "helper");
        write(root, ".agents/skills/pickup.local.md", "fallback");
        assert!(!root.join(".claude").exists());
        assert_eq!(render_skill(root, "pickup").unwrap(), "portable\nlocal\n");
        assert_eq!(
            crate::plan_report::resolve_skill_files(None, root).unwrap(),
            vec![root.join(".agents/skills/pickup/SKILL.md")]
        );
    }

    // trace:BUG-2388 | ai:codex
    #[test]
    fn skill_discovery_precedence_and_legacy_overlays() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, ".claude/skills/SKILL.md", "legacy named SKILL");
        assert_eq!(render_skill(root, "SKILL").unwrap(), "legacy named SKILL");
        std::fs::remove_file(root.join(".claude/skills/SKILL.md")).unwrap();
        write(root, ".claude/skills/pickup.md", "legacy\n");
        write(root, ".claude/skills/pickup.local.md", "claude local");
        assert_eq!(
            render_skill(root, "pickup").unwrap(),
            "legacy\nclaude local"
        );
        write(root, ".claude/skills/pickup/SKILL.md", "folder");
        assert_eq!(
            render_skill(root, "pickup").unwrap(),
            "folder\nclaude local"
        );
        write(root, ".agents/skills/pickup/SKILL.md", "portable\n");
        assert_eq!(render_skill(root, "pickup").unwrap(), "portable\n");
        write(root, ".agents/skills/pickup.local.md", "portable local");
        assert_eq!(
            render_skill(root, "pickup").unwrap(),
            "portable\nportable local"
        );
        assert_eq!(
            discover_skill_files(root).unwrap(),
            vec![
                root.join(".agents/skills/pickup/SKILL.md"),
                root.join(".claude/skills/pickup/SKILL.md"),
                root.join(".claude/skills/pickup.md"),
            ]
        );
    }

    // trace:BUG-2388 | ai:codex
    #[test]
    fn skill_discovery_explicit_paths_missing_and_empty_packs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let error = render_skill(root, "missing").unwrap_err().to_string();
        assert!(error.contains(".agents/skills/") && error.contains(".claude/skills/"));
        std::fs::create_dir_all(root.join(".agents/skills")).unwrap();
        assert!(discover_skill_files(root).unwrap().is_empty());
        assert!(render_skill(root, "missing")
            .unwrap_err()
            .to_string()
            .contains("missing"));
        write(root, "custom.md", "custom");
        assert_eq!(
            crate::plan_report::resolve_skill_files(Some(Path::new("custom.md")), root).unwrap(),
            vec![root.join("custom.md")]
        );
        assert!(
            crate::plan_report::resolve_skill_files(Some(Path::new("absent.md")), root).is_err()
        );
    }

    // trace:BUG-2388 | ai:codex
    #[test]
    fn skill_discovery_sorts_and_follows_repository_skill_links() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, ".agents/skills/z/SKILL.md", "z");
        write(root, "templates/a.md", "linked");
        std::fs::create_dir_all(root.join(".claude/skills/a")).unwrap();
        std::os::unix::fs::symlink(
            "../../../templates/a.md",
            root.join(".claude/skills/a/SKILL.md"),
        )
        .unwrap();
        write(root, ".agents/skills/a/SKILL.md", "a");
        write(root, ".claude/skills/b.md", "b");
        assert_eq!(
            discover_skill_files(root).unwrap(),
            vec![
                root.join(".agents/skills/a/SKILL.md"),
                root.join(".agents/skills/z/SKILL.md"),
                root.join(".claude/skills/a/SKILL.md"),
                root.join(".claude/skills/b.md"),
            ]
        );
        std::fs::remove_file(root.join(".agents/skills/a/SKILL.md")).unwrap();
        assert_eq!(render_skill(root, "a").unwrap(), "linked");
    }
}

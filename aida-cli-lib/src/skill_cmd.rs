//! `aida skill` — render a skill with its local overlay, and lint skills.
// trace:STORY-1728 | ai:claude

use anyhow::Result;

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
            let skills_dir = project_root.join(".claude").join("skills");
            let stock_path = skills_dir.join(format!("{}.md", name));

            if !stock_path.exists() {
                anyhow::bail!("Error: skill '{}' not found under .claude/skills/", name);
            }

            let stock_content = std::fs::read_to_string(&stock_path)
                .map_err(|e| anyhow::anyhow!("Failed to read skill file: {}", e))?;

            let local_path = skills_dir.join(format!("{}.local.md", name));
            if local_path.exists() {
                let local_content = std::fs::read_to_string(&local_path)
                    .map_err(|e| anyhow::anyhow!("Failed to read local skill file: {}", e))?;

                let sep = if stock_content.ends_with('\n') {
                    ""
                } else {
                    "\n"
                };
                print!("{}{}{}", stock_content, sep, local_content);
            } else {
                print!("{}", stock_content);
            }
            return Ok(());
        }
        // `aida skill lint` reads `.claude/skills/*.md` and the plan files
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
        phase: crate::command_registry::Phase::PreStore,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
        },
        run: run_skill,
    };

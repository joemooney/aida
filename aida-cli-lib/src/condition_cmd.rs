//! `aida condition`: check typed conditions (STORY-2402). The condition
//! language and its evaluation live in `aida_core::condition`; this module
//! supplies the live facts (the requirement store, `gh`, git release tags,
//! the clock) and renders the result.
// trace:STORY-2402 | ai:claude

use std::cell::OnceCell;
use std::path::{Path, PathBuf};

use aida_core::condition::{Condition, ConditionFacts, QueryFilter, Version};
use aida_core::{RequirementStatus, RequirementsStore};

#[path = "condition_cmd_args.rs"]
pub(crate) mod condition_cmd_args;
pub(crate) use condition_cmd_args::*;

/// Typed conditions: the machine-checkable form of "when does this come
/// back" for deferrals, relay handoffs and recurrence escalation.
// trace:STORY-2402 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "condition")]
pub(crate) struct ConditionArgs {
    #[clap(subcommand)]
    pub inner: ConditionCommand,
}

/// The live facts a condition is evaluated against. The store loads at most
/// once, and only when a condition needs it.
// trace:STORY-2402 | ai:claude
pub(crate) struct CliConditionFacts {
    project_root: PathBuf,
    store: OnceCell<Option<RequirementsStore>>,
}

impl CliConditionFacts {
    pub(crate) fn new(project_root: &Path) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            store: OnceCell::new(),
        }
    }

    fn store(&self) -> Result<&RequirementsStore, String> {
        self.store
            .get_or_init(|| crate::load_store_for_lookup(&self.project_root))
            .as_ref()
            .ok_or_else(|| "no requirement store found for this project".to_string())
    }
}

impl ConditionFacts for CliConditionFacts {
    fn spec_status(&self, id: &str) -> Result<Option<RequirementStatus>, String> {
        let canonical = crate::spec_arg::canonical(id);
        Ok(self
            .store()?
            .get_requirement_by_spec_id(&canonical)
            .map(|req| req.status.clone()))
    }

    fn pr_merged(&self, number: u64) -> Option<bool> {
        crate::status_prs::gh_pr_is_merged(&self.project_root, number)
    }

    fn latest_release(&self) -> Result<Option<Version>, String> {
        let out = std::process::Command::new("git")
            .current_dir(&self.project_root)
            .args(["tag", "--list"])
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git tag --list failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let tags = String::from_utf8_lossy(&out.stdout);
        Ok(Version::latest_of(tags.lines()))
    }

    fn today(&self) -> chrono::NaiveDate {
        chrono::Utc::now().date_naive()
    }

    fn count(&self, filters: &[QueryFilter]) -> Result<u64, String> {
        Ok(aida_core::condition::count_matching(
            filters,
            &self.store()?.requirements,
        ))
    }
}

/// `aida condition check <CONDITION> [--json]`. A malformed condition or one
/// that cannot be evaluated as written (it names a spec that does not exist)
/// is an error; otherwise the verdict is printed and the command succeeds,
/// whatever the verdict.
// trace:STORY-2402 | ai:claude
fn check(project_root: &Path, text: &str, json: bool) -> anyhow::Result<()> {
    let condition: Condition = text.parse()?;
    let facts = CliConditionFacts::new(project_root);
    let evaluation = condition.evaluate(&facts)?;
    if json {
        let value = serde_json::json!({
            "condition": condition.to_string(),
            "verdict": evaluation.verdict.as_str(),
            "observed": evaluation.observed,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "{condition}: {} ({})",
            evaluation.verdict.as_str(),
            evaluation.observed
        );
    }
    Ok(())
}

// `aida condition` self-loads the requirement store only when a condition
// needs it; no shared storage handle. Read-only.
// trace:STORY-2402 | ai:claude
fn run_condition(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <ConditionArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &args.inner {
        ConditionCommand::Check { condition, json } => {
            let project_root = crate::find_project_root()?;
            check(&project_root, condition, *json)
        }
    }
}

// trace:STORY-2402 | ai:claude
pub(crate) static CONDITION: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "condition",
        help_after: "undefer",
        command: <ConditionArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Read,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_condition),
    };

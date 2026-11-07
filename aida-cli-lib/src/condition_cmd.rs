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
/// once, and only when a condition needs it; sweeps supply their existing snapshot.
// trace:STORY-2402 | ai:claude
pub(crate) struct CliConditionFacts {
    project_root: PathBuf,
    store: OnceCell<Result<RequirementsStore, String>>,
    file: Option<PathBuf>,
}

impl CliConditionFacts {
    pub(crate) fn new(project_root: &Path, file: Option<&str>) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            store: OnceCell::new(),
            file: file.map(PathBuf::from),
        }
    }

    // Reuse a sweep's strict canonical snapshot for spec/query observations.
    // trace:STORY-2402 | ai:codex
    pub(crate) fn from_store(project_root: &Path, store: RequirementsStore) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            store: OnceCell::from(Ok(store)),
            file: None,
        }
    }

    pub(crate) fn store(&self) -> Result<&RequirementsStore, String> {
        self.store
            .get_or_init(|| {
                read_store(&self.project_root, self.file.as_deref()).map_err(|e| format!("{e:#}"))
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

// Strict, read-only reads: an unreadable canonical object must never turn a
// query into a smaller count or select a legacy fallback store.
// trace:STORY-2402 | ai:codex
pub(crate) fn read_store(root: &Path, file: Option<&Path>) -> anyhow::Result<RequirementsStore> {
    let path = match file {
        Some(path) => path.to_path_buf(),
        None => match crate::detect_distributed_store_from(root) {
            Some(path) => path,
            None => aida_core::resolve_requirements_path_in(root, None, None, Path::new(""))?,
        },
    };
    if path.is_dir() {
        // Unit fixtures must not replace the process-wide alias map, literal-ID
        // check or ID widths used by parallel tests. These fixtures use literal
        // IDs, so they need only the strict object read below. Production and
        // binary integration tests always install the store's alias context.
        // trace:STORY-2402 | ai:codex
        if !cfg!(test) {
            aida_core::node_alias::install_from_store(&path);
        }
        let objects = path.join("objects");
        let mut store = RequirementsStore::new();
        store.requirements = aida_core::object_store::list_objects_strict(&objects)?
            .into_iter()
            .map(|(_, path)| aida_core::object_store::read_object_from_path(&path))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(store)
    } else {
        anyhow::ensure!(
            !matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("db" | "sqlite" | "sqlite3")
            ),
            "condition checks require canonical/YAML rows; migrate the archived SQLite store first"
        );
        Ok(serde_yaml::from_reader(std::fs::File::open(path)?)?)
    }
}

impl ConditionFacts for CliConditionFacts {
    fn spec_status(&self, id: &str) -> Result<Option<RequirementStatus>, String> {
        // Resolve literal IDs before aliases, and refuse ambiguous agreed IDs.
        // trace:STORY-2402 | ai:codex
        self.store()?
            .get_requirement_unambiguous(id)
            .map(|req| req.map(|req| req.status.clone()))
            .map_err(|e| e.to_string())
    }

    fn pr_merged(&self, number: u64) -> Option<bool> {
        crate::forge::observe_github_pr_merged(&self.project_root, number).known()
    }

    fn latest_release(&self) -> Result<Option<Version>, String> {
        let mut command = std::process::Command::new("git");
        command
            .current_dir(&self.project_root)
            .args(["tag", "--list"]);
        let out = crate::command_output_with_timeout(command, std::time::Duration::from_secs(15))
            .ok_or_else(|| "git tag probe failed or timed out".to_string())?;
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
fn check(project_root: &Path, file: Option<&str>, text: &str, json: bool) -> anyhow::Result<()> {
    let condition: Condition = text.parse()?;
    let facts = CliConditionFacts::new(project_root, file);
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
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <ConditionArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &args.inner {
        ConditionCommand::Check { condition, json } => {
            let project_root = crate::find_project_root()?;
            check(
                &project_root,
                ctx.globals.and_then(|g| g.file.as_deref()),
                condition,
                *json || crate::output_format_is_json(),
            )
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

#[cfg(test)]
#[path = "tests/story_2402_condition_cmd_tests.rs"]
mod tests;

// Shared admission for deferrals, handoff triggers and recurrence.
// trace:STORY-2402 | ai:codex
pub(crate) fn canonical_condition(
    until: Option<&str>,
    reason: Option<&str>,
) -> anyhow::Result<Option<String>> {
    use aida_core::condition::Condition;
    let Some(raw) = until else { return Ok(None) };
    let condition: Condition = raw.parse().map_err(|err| {
        anyhow::anyhow!(
            "invalid --until condition: {err}. For a human decision use manual:<who> with --reason"
        )
    })?;
    if matches!(condition, Condition::Manual { .. })
        && reason.is_none_or(|reason| reason.trim().is_empty())
    {
        anyhow::bail!("manual conditions require --reason with a non-empty explanation");
    }
    Ok(Some(condition.to_string()))
}

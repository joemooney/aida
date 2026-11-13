//! `aida integration`: the integration-branch flow (STORY-2737). Work lands
//! on `integration` with minimal ceremony; `main` is the batched,
//! fully-verified checkpoint that completes specs.
//!
//! `aida integration tags` keeps the `landed-in-integration` tag honest so
//! the open list shows which specs are done but not yet promoted. It reads
//! commit subjects with the same trailer parser merge-time completion uses,
//! so a `(Refs X)` commit never marks `X` landed. Writes go through
//! `aida edit --add-tag/--remove-tag`, the one validated tag-write path.
// trace:STORY-2737 | ai:claude

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

#[path = "integration_cmd_args.rs"]
pub(crate) mod integration_cmd_args;
pub(crate) use integration_cmd_args::*;

/// The tag on a spec whose completing commit is on integration, not main.
pub(crate) const LANDED_TAG: &str = "landed-in-integration";

/// Run `git` in `root`, returning trimmed stdout on success.
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    anyhow::ensure!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// The spec ids the commit `subjects` complete, upper-cased and deduplicated
/// by numeric identity, in first-seen order.
pub(crate) fn completing_ids(subjects: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for subject in subjects {
        for id in crate::zen_drive_root::extract_spec_ids_from_commit(subject) {
            let id = id.to_ascii_uppercase();
            if !out
                .iter()
                .any(|seen| aida_core::id_identity::same_id(seen, &id))
            {
                out.push(id);
            }
        }
    }
    out
}

/// One spec as `aida list --format json` reports it.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct ListedSpec {
    pub display_id: String,
    pub status: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn is_open(status: &str) -> bool {
    matches!(
        status,
        "Draft" | "Approved" | "Planned" | "InProgress" | "NeedsAttention"
    )
}

/// The tag edits that make `specs` agree with `landed` (the ids completed on
/// integration but not on main): add the tag to every open landed spec that
/// lacks it; remove it from every tagged spec that is closed or not landed.
/// Returned sorted, as (adds, removes).
pub(crate) fn plan_tag_changes(
    specs: &[ListedSpec],
    landed: &[String],
) -> (Vec<String>, Vec<String>) {
    let is_landed = |id: &str| {
        landed
            .iter()
            .any(|l| aida_core::id_identity::same_id(l, id))
    };
    let mut adds = Vec::new();
    let mut removes = Vec::new();
    for spec in specs {
        let tagged = spec.tags.iter().any(|t| t == LANDED_TAG);
        let wanted = is_open(&spec.status) && is_landed(&spec.display_id);
        if wanted && !tagged {
            adds.push(spec.display_id.clone());
        } else if tagged && !wanted {
            removes.push(spec.display_id.clone());
        }
    }
    adds.sort();
    removes.sort();
    (adds, removes)
}

fn aida_exe() -> std::path::PathBuf {
    crate::aida_bin::current_executable_path()
        .unwrap_or_else(crate::aida_bin::running_executable_fallback)
}

fn list_specs(root: &Path, extra: &[&str]) -> Result<Vec<ListedSpec>> {
    let out = Command::new(aida_exe())
        .current_dir(root)
        .args([
            "list",
            "--no-focus",
            "--all",
            "--limit",
            "0",
            "--format",
            "json",
        ])
        .args(extra)
        .stdin(Stdio::null())
        .output()
        .context("running aida list")?;
    anyhow::ensure!(
        out.status.success(),
        "aida list failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    serde_json::from_slice(&out.stdout).context("parsing aida list --format json")
}

fn run_tags(root: &Path, dry_run: bool) -> Result<()> {
    for r in [
        "refs/remotes/origin/main",
        "refs/remotes/origin/integration",
    ] {
        git(root, &["rev-parse", "--verify", "-q", r])
            .with_context(|| format!("`{r}` is missing; run `git fetch origin` first"))?;
    }
    let log = git(
        root,
        &[
            "log",
            "--format=%s",
            "refs/remotes/origin/main..refs/remotes/origin/integration",
        ],
    )?;
    let subjects: Vec<&str> = log.lines().collect();
    let landed = completing_ids(&subjects);
    // Open specs (candidates to tag) plus every spec already tagged, open or
    // closed (candidates to untag).
    let mut specs = list_specs(root, &["--status", "open"])?;
    for tagged in list_specs(root, &["--tags", LANDED_TAG])? {
        if !specs.iter().any(|s| s.display_id == tagged.display_id) {
            specs.push(tagged);
        }
    }
    let (adds, removes) = plan_tag_changes(&specs, &landed);
    println!(
        "{} commit(s) on origin/integration not on origin/main complete {} spec(s).",
        subjects.len(),
        landed.len()
    );
    if adds.is_empty() && removes.is_empty() {
        println!("`{LANDED_TAG}` tags are up to date.");
        return Ok(());
    }
    for (verb, flag, ids) in [
        ("tag", "--add-tag", &adds),
        ("untag", "--remove-tag", &removes),
    ] {
        for id in ids {
            if dry_run {
                println!("  would {verb} {id}");
                continue;
            }
            let status = Command::new(aida_exe())
                .current_dir(root)
                .args(["edit", id, flag, LANDED_TAG])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .status()
                .with_context(|| format!("running aida edit {id}"))?;
            anyhow::ensure!(
                status.success(),
                "aida edit {id} {flag} {LANDED_TAG} failed"
            );
            println!("  {verb}ged {id}");
        }
    }
    Ok(())
}

/// The integration-branch flow: `integration` takes work fast; `main` is the
/// batched checkpoint that completes specs.
// trace:STORY-2737 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "integration")]
pub(crate) struct IntegrationArgs {
    #[clap(subcommand)]
    pub command: IntegrationCommand,
}

fn run_integration(matches: &clap::ArgMatches, _ctx: &crate::cmdkit::DispatchCtx) -> Result<()> {
    let args = <IntegrationArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let root = std::env::current_dir().context("reading the current directory")?;
    match args.command {
        IntegrationCommand::Tags { dry_run } => run_tags(&root, dry_run),
    }
}

// The tag writes go through `aida edit`, which carries its own policy.
// trace:STORY-2737 | ai:claude
pub(crate) static INTEGRATION: crate::cmdkit::CommandReg = crate::cmdkit::CommandReg {
    name: "integration",
    help_after: "integrate",
    command: <IntegrationArgs as clap::CommandFactory>::command,
    policy: |_| crate::cmdkit::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::cmdkit::Handler::Early(run_integration),
};

#[cfg(test)]
#[path = "tests/story_2737_integration_tags_tests.rs"]
mod tests;

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
/// only as literal tokens, in first-seen order. Padding-equal tokens
/// (`BUG-9`, `BUG-09`) stay distinct: each may name its own stored spec, so
/// identity is deduplicated after [`resolve_trailer`], by UUID.
// trace:STORY-2737 | ai:claude
pub(crate) fn completing_ids(subjects: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for subject in subjects {
        for id in crate::zen_drive_root::extract_spec_ids_from_commit(subject) {
            let id = id.to_ascii_uppercase();
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// One stored spec, as the tag plan sees it: its stable identity (the
/// UUID `aida edit` resolves directly), the id shown to the operator, its
/// status and tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListedSpec {
    pub uuid: uuid::Uuid,
    pub display_id: String,
    pub status: aida_core::RequirementStatus,
    pub tags: Vec<String>,
}

impl ListedSpec {
    fn of(req: &aida_core::Requirement) -> Self {
        let mut tags: Vec<String> = req.tags.iter().cloned().collect();
        tags.sort();
        Self {
            uuid: req.id,
            display_id: req.display_id(),
            status: req.status.clone(),
            tags,
        }
    }
}

/// Whether a spec in `status` can still be awaiting promotion to main. Done
/// is the normal state of implemented work whose completing commit is on
/// integration only, so it keeps (and gains) the tag until promotion
/// completes it.
// trace:STORY-2737 | ai:claude
fn awaits_promotion(status: &aida_core::RequirementStatus) -> bool {
    use aida_core::RequirementStatus as S;
    matches!(
        status,
        S::Draft | S::Approved | S::Planned | S::InProgress | S::NeedsAttention | S::Done
    )
}

/// The stored spec a commit trailer token names: the exact literal id
/// (native `spec_id`, then agreed id, compared as strings, never by numeric
/// identity), then its node-alias expansion, and a numeric-padding match
/// (`TASK-0042`) last, only when it is unique. Origin ids are native
/// `spec_id`s, so a pre-agreement trailer resolves to the same spec as its
/// agreed display id. `None` for a token naming no spec, or one whose
/// padding matches two stored specs.
// trace:STORY-2737 | ai:claude
pub(crate) fn resolve_trailer(
    store: &aida_core::RequirementsStore,
    token: &str,
) -> Option<uuid::Uuid> {
    let literal = |id: &str| {
        let exact = |field: &Option<String>| {
            field
                .as_deref()
                .is_some_and(|stored| stored.eq_ignore_ascii_case(id))
        };
        let native = store.requirements.iter().find(|r| exact(&r.spec_id));
        native
            .or_else(|| store.requirements.iter().find(|r| exact(&r.agreed_id)))
            .map(|r| r.id)
    };
    aida_core::node_alias::resolve_literal_first_opt(token, literal).or_else(|| {
        let mut hits = store.requirements.iter().filter(|r| {
            [Some(r.display_id()), r.spec_id.clone(), r.agreed_id.clone()]
                .into_iter()
                .flatten()
                .any(|id| aida_core::id_identity::same_id(&id, token))
        });
        // Padding-equal ids that name two specs are ambiguous: tag neither.
        match (hits.next(), hits.next()) {
            (Some(one), None) => Some(one.id),
            _ => None,
        }
    })
}

/// The tag edits that make `specs` agree with `landed` (the stored specs
/// completed on integration but not on main): add the tag to every open
/// landed spec that lacks it; remove it from every tagged spec that is
/// closed or not landed. Returned sorted by display id, as (adds, removes).
pub(crate) fn plan_tag_changes<'a>(
    specs: &'a [ListedSpec],
    landed: &std::collections::HashSet<uuid::Uuid>,
) -> (Vec<&'a ListedSpec>, Vec<&'a ListedSpec>) {
    let mut adds = Vec::new();
    let mut removes = Vec::new();
    for spec in specs {
        let tagged = spec.tags.iter().any(|t| t == LANDED_TAG);
        let wanted = awaits_promotion(&spec.status) && landed.contains(&spec.uuid);
        if wanted && !tagged {
            adds.push(spec);
        } else if tagged && !wanted {
            removes.push(spec);
        }
    }
    adds.sort_by(|a, b| a.display_id.cmp(&b.display_id));
    removes.sort_by(|a, b| a.display_id.cmp(&b.display_id));
    (adds, removes)
}

/// Every stored spec, read from the store itself: no list query, so no
/// role scope, focus or status filter can hide a landed or tagged spec.
// trace:STORY-2737 | ai:claude
fn load_store(root: &Path) -> Result<aida_core::RequirementsStore> {
    let path = aida_core::store_locate::detect_distributed_store_from(root)
        .unwrap_or_else(|| root.join(".aida-store"));
    aida_core::Storage::new(path)
        .load()
        .context("loading the requirements store")
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
    let store = load_store(root)?;
    let landed_specs: std::collections::HashSet<uuid::Uuid> = landed
        .iter()
        .filter_map(|token| resolve_trailer(&store, token))
        .collect();
    let specs: Vec<ListedSpec> = store.requirements.iter().map(ListedSpec::of).collect();
    let (adds, removes) = plan_tag_changes(&specs, &landed_specs);
    println!(
        "{} commit(s) on origin/integration not on origin/main complete {} spec(s).",
        subjects.len(),
        landed_specs.len()
    );
    if adds.is_empty() && removes.is_empty() {
        println!("`{LANDED_TAG}` tags are up to date.");
        return Ok(());
    }
    for (verb, flag, specs) in [
        ("tag", "--add-tag", &adds),
        ("untag", "--remove-tag", &removes),
    ] {
        for spec in specs {
            let id = &spec.display_id;
            if dry_run {
                println!("  would {verb} {id}");
                continue;
            }
            // Edit by UUID: the stable identity, never an alias guess.
            let uuid = spec.uuid.to_string();
            let status = Command::new(crate::kernel::aida_exe_path())
                .current_dir(root)
                .args(["edit", &uuid, flag, LANDED_TAG])
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

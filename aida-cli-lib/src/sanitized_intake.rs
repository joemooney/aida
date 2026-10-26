//! `aida add --sanitized`: generic intake for a clone with an enabled WORK
//! transport profile. `--template` and `--preview` are read-only: they never
//! open the store, allocate an identifier, persist counters or commit. A write
//! goes through the backend's prepared path, which rechecks everything.
// trace:STORY-1742 | ai:claude
use crate::command_registry::{Flow, StoreCtx};
use crate::requirements_cli::{parse_requirement_type, resolve_description, AddArgs};
use aida_core::transport_policy::PreviewVerdict;
use aida_core::{DatabaseBackend, Requirement, RequirementPriority, RequirementStatus};
use anyhow::{bail, Context, Result};

/// Generic description scaffold. Synthetic placeholders only.
pub(crate) const TEMPLATE: &str = "\
## Problem

<What fails, described generically. No names, hosts, addresses, paths or logs.>

## Reproduction

1. <Generic step>
2. <Generic step>

## Expected

<What should happen.>

## Actual

<What happens instead.>
";

/// The flags sanitized intake never honors: each would add caller identity,
/// workflow context or a second write beyond the single generic row.
fn refuse_extras(args: &AddArgs) -> Result<()> {
    let extras = [
        ("--owner", args.owner.is_some()),
        ("--feature", args.feature.is_some()),
        ("--tags", args.tags.is_some()),
        ("--prefix", args.prefix.is_some()),
        ("--parent", args.parent.is_some()),
        ("--blocked-by", !args.blocked_by.is_empty()),
        ("--force-parent", args.force_parent),
        ("--interactive", args.interactive),
        ("--human-only", args.human_only),
        ("--no-human-only", args.no_human_only),
        ("--effort", args.effort.is_some()),
        ("--weight", args.weight.is_some()),
        ("--mode", args.mode.is_some()),
        ("--queue", args.queue),
        ("--batch", args.batch.is_some()),
        ("--for", args.r#for.is_some()),
        ("--gates", !args.gates.is_empty()),
    ];
    if let Some((flag, _)) = extras.iter().find(|(_, set)| *set) {
        bail!(
            "{flag} is not available with --sanitized: sanitized intake files only a generic \
             title, description, type and priority."
        );
    }
    if args
        .status
        .as_deref()
        .is_some_and(|status| !status.eq_ignore_ascii_case("draft"))
    {
        bail!("--sanitized always files a draft; omit --status.");
    }
    Ok(())
}

/// The single generic row. Errors never echo the supplied text.
fn candidate(args: &AddArgs) -> Result<Requirement> {
    refuse_extras(args)?;
    let title = args
        .title
        .clone()
        .or_else(|| args.title_positional.clone())
        .filter(|title| !title.trim().is_empty())
        .context("--sanitized requires a non-blank title.")?;
    if title.chars().count() > 200 {
        bail!("--sanitized titles are limited to 200 characters.");
    }
    let description = resolve_description(
        &args.description,
        &args.description_from_file,
        args.description_stdin,
    )?
    .unwrap_or_default();
    let mut row = Requirement::new(title, description);
    row.req_type = parse_requirement_type(args.r#type.as_deref().unwrap_or("bug"))?;
    row.status = RequirementStatus::Draft;
    row.priority = match args.priority.as_deref().map(str::to_ascii_lowercase) {
        None => RequirementPriority::Medium,
        Some(p) if p == "high" => RequirementPriority::High,
        Some(p) if p == "medium" => RequirementPriority::Medium,
        Some(p) if p == "low" => RequirementPriority::Low,
        Some(_) => bail!("--priority must be high, medium or low."),
    };
    Ok(row)
}

fn preview(args: &AddArgs) -> Result<()> {
    let row = candidate(args)?;
    let root = std::env::current_dir().context("current directory unavailable")?;
    let verdict = aida_core::transport_policy::preview(&root, &[&row.title, &row.description])?;
    println!("Sanitized intake preview. Nothing was written and no identifier was assigned.");
    match verdict {
        PreviewVerdict::Refused(category) => {
            // Never echo a refused candidate.
            println!("Policy: refused ({category}). Revise the text and preview again.");
            return Ok(());
        }
        PreviewVerdict::Clean => println!("Policy: passes this clone's reviewed policy."),
        PreviewVerdict::NotEnrolled => println!(
            "Policy: this clone has no enabled WORK transport profile; a sanitized write would be refused."
        ),
    }
    println!(
        "  type: {:?}  status: draft  priority: {:?}",
        row.req_type, row.priority
    );
    println!("  title: {}", row.title);
    println!("  description: {} line(s)", row.description.lines().count());
    println!(
        "This preview is for your review only. The complete write, including its identifier, \
         metadata, history and commit identity, is checked again when you file it."
    );
    Ok(())
}

fn write(
    store_path: &std::path::Path,
    backend: &dyn DatabaseBackend,
    args: &AddArgs,
) -> Result<()> {
    let row = candidate(args)?;
    if !aida_core::transport_policy::enrolled(store_path)? {
        bail!(
            "--sanitized writes require an enabled WORK transport profile on this clone. \
             --template and --preview work anywhere."
        );
    }
    let added = backend.add_requirement(row)?;
    println!(
        "Added: {}",
        added.spec_id.as_deref().unwrap_or("(no identifier)")
    );
    Ok(())
}

/// Dispatch for every phase: template and preview finish before storage is
/// opened; a write runs only against the attached git store.
pub(crate) fn run(args: &AddArgs, store: &Option<StoreCtx>) -> Result<Flow> {
    match store {
        None if args.template => {
            print!("{TEMPLATE}");
            Ok(Flow::Return)
        }
        None if args.preview => preview(args).map(|()| Flow::Return),
        None => {
            // Refuse unsupported flags before any storage is opened.
            candidate(args)?;
            Ok(Flow::Continue)
        }
        Some(StoreCtx::Distributed { .. }) => Ok(Flow::Continue),
        Some(StoreCtx::Git {
            store_path,
            backend,
        }) => write(store_path, *backend, args).map(|()| Flow::Return),
        Some(_) => bail!("--sanitized requires the git-canonical requirements store."),
    }
}

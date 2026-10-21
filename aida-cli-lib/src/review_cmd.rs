//! `aida review` — drive human review of a held spec.
// trace:STORY-1728 | ai:claude

use anyhow::Result;

// trace:STORY-1728 | ai:claude
#[path = "review_cmd_args.rs"]
pub(crate) mod review_cmd_args;
pub(crate) use review_cmd_args::*;

/// Drive human review of a held spec, or generate review prompts.
///
/// `aida review <SPEC>` is the human-review counterpart to `aida queue
/// work`: it locates the spec's review surface (an open draft PR, else
/// the branch + commits, else "built locally, never pushed"), runs a
/// reviewer over the diff against the spec's `## Acceptance` criteria,
/// then presents the verdict and lets you decide — approve, request
/// changes, open the diff, or defer. It never auto-merges.
///
/// The `prompt` / `assemble` subcommands are the review-prompt helpers.
// trace:STORY-67 | ai:claude
// trace:STORY-553 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "review")]
pub(crate) struct ReviewArgs {
    /// Spec to review (a story, task, or bug id). Resolves its review surface,
    /// runs a reviewer over the diff, and prompts you to decide. Omit
    /// when using a `prompt` / `assemble` subcommand.
    pub spec: Option<String>,

    /// Skip the reviewer-agent analysis: just locate + report the
    /// review surface and the recommended next command. Useful in
    /// non-interactive contexts or when you only want the diff pointer.
    #[clap(long)]
    pub no_agent: bool,

    /// Skip the stale-base warning. By default, when the spec's PR is
    /// behind its base branch, the review prints a warning naming the
    /// gap and the rebase command (the review still proceeds either
    /// way). Pass this flag when a deliberately stale review is
    /// intended — same opt-out the reviewer-role pickup honors.
    // trace:BUG-510 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(long)]
    pub allow_stale_base: bool,

    /// Explicit base branch to target when AC-5 offers to open a
    /// change from a branch with no open PR/MR. Always wins over any
    /// base saved from a previous offer (`.aida/mr-recovery/<spec>.json`)
    /// — use this to correct a stuck recovery, e.g. when a GitLab
    /// project's default branch became the feature branch itself
    /// because remote `main` was never pushed.
    // trace:BUG-1610 | ai:claude
    #[clap(long, value_name = "BRANCH")]
    pub target_branch: Option<String>,

    #[clap(subcommand)]
    pub cmd: Option<crate::cli::ReviewCommand>,
}

fn run_review(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <ReviewArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                ReviewArgs {
                    spec,
                    no_agent,
                    allow_stale_base,
                    target_branch,
                    cmd,
                } => {
                    // trace:STORY-553 | ai:claude — `aida review <SPEC>` drives a
                    // human-decision review of a held spec (the dual of `aida queue
                    // work`). The `prompt` / `assemble` subcommands stay the
                    // review-prompt helpers, read via the same Storage façade.
                    match (spec, cmd) {
                        (Some(spec_id), _) => {
                            handle_review_spec(
                                backend,
                                store_path,
                                spec_id,
                                *no_agent,
                                *allow_stale_base,
                                target_branch.as_deref(),
                            )?;
                        }
                        (None, Some(review_cmd)) => {
                            let storage = Storage::new(store_path);
                            handle_review_command(review_cmd, &storage)?;
                        }
                        (None, None) => {
                            anyhow::bail!("pass a spec id (`aida review <SPEC>`) or a subcommand (`prompt` / `assemble`)");
                        }
                    }
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                ReviewArgs {
                    spec,
                    no_agent,
                    allow_stale_base,
                    target_branch,
                    cmd,
                } => {
                    // trace:STORY-553 | ai:claude — `aida review <SPEC>` drives the
                    // human-decision review; `prompt` / `assemble` stay the helper
                    // subcommands. The legacy (SQLite) path supports the prompt
                    // helpers only; the spec-review verb needs the git-canonical
                    // backend's surface resolution, so guide the user there.
                    match (spec, cmd) {
                        (Some(_), _) => anyhow::bail!(
                            "`aida review <SPEC>` requires the default git-canonical \
                         backend; the deprecated --centralized SQLite mode does \
                         not support it."
                        ),
                        (None, Some(review_cmd)) => handle_review_command(review_cmd, &storage)?,
                        (None, None) => {
                            let _ = (no_agent, allow_stale_base, target_branch);
                            anyhow::bail!("pass a spec id (`aida review <SPEC>`) or a subcommand (`prompt` / `assemble`)");
                        }
                    }
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static REVIEW: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "review",
        help_after: "trace",
        command: <ReviewArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_review),
    };
// trace:STORY-1728 | ai:claude
pub(crate) fn handle_review_command(
    cmd: &crate::ReviewCommand,
    storage: &crate::Storage,
) -> anyhow::Result<()> {
    use crate::*;
    match cmd {
        // trace:TASK-1993 | ai:codex
        ReviewCommand::Watch(args) => crate::review_watch::run(args),
        ReviewCommand::Prompt {
            specs,
            pr,
            forge,
            write,
        } => generate_review_prompt(
            storage,
            specs.as_deref(),
            *pr,
            forge.as_deref(),
            write.as_deref(),
        ),
        ReviewCommand::Assemble { output } => {
            let project_root = find_project_root()?;
            let out_path = rules_sync::assemble_review_md(&project_root, output.as_deref())?;
            println!(
                "{} Assembled root REVIEW.md at {}",
                crate::glyph(crate::glyphs::Glyph::Check),
                out_path.display()
            );
            Ok(())
        }
        // trace:BUG-775 | ai:claude
        ReviewCommand::Record {
            spec,
            verdict,
            sha,
            branch,
            summary,
            finding,
            finding_class,
            pr,
        } => handle_review_record(
            spec,
            verdict,
            sha.as_deref(),
            branch.as_deref(),
            summary.as_deref(),
            finding,
            finding_class,
            *pr,
        ),
        // trace:STORY-1417 | ai:claude
        ReviewCommand::Classes { since, json } => handle_review_classes(since.as_deref(), *json),
        // trace:STORY-1405 | ai:claude
        ReviewCommand::Claim {
            pr,
            sha,
            spec,
            ttl_mins,
            release,
        } => handle_review_claim(*pr, sha.as_deref(), spec.as_deref(), *ttl_mins, *release),
        // trace:BUG-775 | ai:claude
        ReviewCommand::Verdict { spec, json } => handle_review_verdict_show(spec, *json),
        ReviewCommand::List { json } => handle_review_list(*json),
        // trace:BUG-1516 | ai:claude
        ReviewCommand::NormalizeShas { dry_run } => handle_review_normalize_shas(*dry_run),
        // trace:TASK-1307 | ai:claude
        // trace:TASK-1423 | ai:claude
        ReviewCommand::Stranded { json, fix, age } => handle_review_stranded(*json, *fix, *age),
        // trace:STORY-1415 | ai:claude
        ReviewCommand::Mode { mode } => match mode {
            cli::ReviewModeCommand::MassChange { action, days, json } => {
                mass_change::handle_mass_change_command(action.as_str(), *days, *json)
            }
        },
    }
}

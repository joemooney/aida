//! `aida human` command handler — the 'human' role-vector front door (SPIKE-57,
//! Phase 2): the operator "awaiting you" bottleneck view. Extracted verbatim
//! from `main.rs` (SPIKE-78, pure movement — no behavior change). The shared
//! aggregation lives in `crate::handle_list_human`, which BOTH this front door
//! and the STORY-562 `aida list human` view delegate to, so it stays in
//! `main.rs`.

use anyhow::Result;

// trace:STORY-1728 | ai:claude
#[path = "human_cmd_args.rs"]
pub(crate) mod human_cmd_args;
pub(crate) use human_cmd_args::*;

use crate::{find_project_root, handle_list_human, last_drain};

/// STORY-562: `aida list human` (and `aida list --human`) — the discoverable
/// "what needs me?" view. The DATA already exists: `burndown explain`
/// classifies every open spec into a bucket and flags which `needs_human()`.
/// This surfaces exactly that human-attention subset under the operator's
/// instinct (`list human`, sibling to the `open`/`closed` status aliases),
/// grouped by reason so it reads as a triage list. We REUSE the
/// [`burndown::explain_reasons`] classifier verbatim — no re-derived buckets.
/// `short` prints just the IDs (composes with `aida list --short`).
// trace:STORY-562 | ai:claude
/// `aida human` — the 'human' role-vector front door (SPIKE-57, Phase 2).
///
/// Bare `aida human` surfaces the bottleneck view: every spec classified
/// human-required (the canonical `burndown::human_required` predicate),
/// grouped by WHY. It is the role-vector entry point that converges with the
/// STORY-562 `aida list human` view rather than competing with it — both
/// delegate to the single `handle_list_human` implementation, so the two front
/// doors can never drift.
///
/// Presence (`home`/`away`/`status`) and `--for human` routing are later phases
/// of SPIKE-57; this verb is just the front door + the named predicate today.
// trace:TASK-746 | ai:claude
pub(crate) fn handle_human_command(
    short: bool,
    backend: &aida_core::CachedGitBackend,
) -> Result<()> {
    // STORY-730: the morning-after banner on `aida status` points the operator at
    // `aida human` — running it means they have looked, so acknowledge the last
    // drain outcome and the banner stops nagging. Best-effort. trace:STORY-730
    if let Ok(root) = find_project_root() {
        last_drain::acknowledge(&root);
    }
    handle_list_human(short, backend)
}

/// The 'human' role vector — the front door to "what needs a person?".
/// Bare `aida human [--short]` shows the bottleneck view (every spec a
/// human must decide, review, or triage), the same set as `aida list
/// human`, grouped by WHY. `aida human unblock` emits a paste-ready advisor
/// prompt that grooms the open items keeping themselves out of the
/// burndown. `aida human away/home/presence` names the operator-presence
/// verbs under the same role vector while keeping the top-level aliases.
/// The human is the permanent terminus of the escalation cascade
/// (implementer → advisor → human); this gives that role a first-class home
/// symmetric with the agent roles.
// trace:TASK-746 trace:STORY-563 | ai:claude — plain `//` keeps it out of `--help`.
#[derive(clap::Parser, Debug)]
#[clap(name = "human")]
pub(crate) struct HumanArgs {
    /// Bare spec-ids, one per line — usable in `$(...)` / xargs (mirrors
    /// `aida list human --short`). Ignored when a subcommand is given.
    // trace:TASK-746 | ai:claude
    #[clap(long)]
    pub short: bool,
    #[clap(subcommand)]
    pub command: Option<crate::cli::HumanCommand>,
}

fn run_human(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <HumanArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => {
            // STORY-563: `aida human <subcommand>` self-loads the store like `aida why`
            // / `burndown explain`, or delegates to the top-level presence handlers.
            // Dispatch early, no shared storage handle needed.
            // trace:STORY-563 | ai:claude
            // Presence + unblock subcommands need no storage handle, so dispatch them
            // here before store init. The STORY-611 action aliases (`answer`/`review`/
            // `decide`) DO need the backend — they fall through to the main dispatch.
            // trace:STORY-611 | ai:claude
            if let HumanArgs {
                command: Some(human_cmd),
                ..
            } = &args
            {
                if human_subcommand_needs_no_storage(human_cmd) {
                    return crate::command_registry::returned(handle_human_subcommand(human_cmd));
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
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
                // Bare `aida human [--short]` lists the three pending-for-human
                // buckets. The presence/unblock subcommands already returned via the
                // early pre-storage dispatch; the STORY-611 action aliases
                // (`answer`/`review`/`decide`) reach here because they need the
                // backend, and delegate to their canonical verbs.
                // trace:TASK-746 trace:STORY-563 trace:STORY-611
                HumanArgs { short, command } => match command {
                    // STORY-611: `aida human answer|decide <spec> <choice>` →
                    // `aida questions answer` (pure pass-through).
                    Some(cli::HumanCommand::Answer { spec, choice, note })
                    | Some(cli::HumanCommand::Decide { spec, choice, note }) => {
                        questions_answer_one(backend, store_path, spec, choice, note.as_deref())?;
                    }
                    // STORY-611: `aida human review <spec>` → `aida review <spec>`.
                    Some(cli::HumanCommand::Review {
                        spec,
                        no_agent,
                        allow_stale_base,
                        guided,
                    }) => {
                        if *guided {
                            handle_guided_human_review(spec)?;
                        } else {
                            handle_review_spec(
                                backend,
                                store_path,
                                spec,
                                *no_agent,
                                *allow_stale_base,
                                None,
                            )?;
                        }
                    }
                    _ => {
                        self::handle_human_command(*short, backend)?;
                    }
                },
                #[allow(unreachable_patterns)]
                _ => crate::git_backend_cmd::exit_unsupported_on_git_backend(),
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                HumanArgs { .. } => {
                    // The `aida human` bottleneck view reads the git-canonical store
                    // (the burndown classifier needs the relationship graph + tags);
                    // the deprecated --centralized backend can't back it. trace:TASK-746
                    anyhow::bail!(
                        "aida human requires the distributed git-canonical store \
                     (run `aida init` to migrate, or this project is on the \
                     deprecated --centralized backend)"
                    );
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!("`aida human` finished before storage init"),
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static HUMAN: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "human",
        help_after: "goal",
        command: <HumanArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_human),
    };

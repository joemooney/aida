//! `aida goal` command cluster (TASK-242), lifted out of `main.rs` (SPIKE-78).
//!
//! Derives a machine-checkable `/goal` completion condition from AIDA metadata
//! — a batch, an epic's children, a spec, a PR, or a role queue — where each
//! flag contributes one AND-composed clause carrying its own deterministic
//! verify command. Pure formatting over the clause vocabulary; `--copy` /
//! `--invoke` / deep-link output reach shared helpers in `crate::`. Extracted
//! verbatim; no behavior change.

use anyhow::Result;
use colored::Colorize;

use crate::copy_to_clipboard;

// The advisor's standing goal. trace:STORY-2243 | ai:claude
#[path = "goal_advisor.rs"]
pub(crate) mod goal_advisor;
use crate::deep_link;
use crate::forge::ForgeKind;

// ────────────────────────────────────────────────────────────────────
// TASK-242: `aida goal` — derive machine-checkable completion conditions
// from AIDA metadata for /goal, /schedule, and autonomous-mode runs.
// trace:TASK-242 | ai:claude
// ────────────────────────────────────────────────────────────────────

/// One clause of a `/goal` completion condition: a human-readable
/// statement plus an explicit, deterministic command to verify it.
pub(crate) struct GoalClause {
    description: String,
    pub(crate) verify: String,
    /// What the clause checks and its operand, so a stored goal can be
    /// evaluated (`aida goal advisor check`). trace:STORY-2243 | ai:claude
    kind: aida_core::advisor_goal::ClauseKind,
    arg: String,
}

impl GoalClause {
    /// The clause as stored in an advisor goal. trace:STORY-2243 | ai:claude
    pub(crate) fn to_record(&self) -> aida_core::advisor_goal::GoalClauseRecord {
        aida_core::advisor_goal::GoalClauseRecord {
            kind: self.kind.clone(),
            arg: self.arg.clone(),
            description: self.description.clone(),
            verify: self.verify.clone(),
        }
    }
}

/// Build the ordered list of `GoalClause`s from the `aida goal` flags.
/// Pure — no store/IO — so the condition vocabulary is unit-testable.
/// Bails when no condition flag is given. Project-aware callers pass the
/// resolved forge so GitLab `/goal --pr` clauses say MR/glab rather than PR/gh.
// trace:TASK-242 trace:TASK-1240 | ai:claude+codex
pub(crate) fn build_goal_clauses_for_forge(
    batch: Option<&str>,
    epic: Option<&str>,
    spec: Option<&str>,
    pr: Option<u64>,
    queue_empty: Option<&str>,
    forge: ForgeKind,
) -> Result<Vec<GoalClause>> {
    use aida_core::advisor_goal::ClauseKind;
    let mut clauses: Vec<GoalClause> = Vec::new();

    if let Some(name) = batch {
        let name = name.strip_prefix("batch:").unwrap_or(name);
        clauses.push(GoalClause {
            description: format!(
                "all specs tagged `batch:{}` are resolved (Completed or Rejected)",
                name
            ),
            verify: format!("`aida list --tags batch:{}` returns no rows", name),
            kind: ClauseKind::Batch,
            arg: name.to_string(),
        });
    }
    if let Some(id) = epic {
        clauses.push(GoalClause {
            description: format!(
                "all direct children of {} are resolved (Completed or Rejected)",
                id
            ),
            verify: format!("`aida list --parent {}` returns no rows", id),
            kind: ClauseKind::Epic,
            arg: id.to_string(),
        });
    }
    if let Some(id) = spec {
        clauses.push(GoalClause {
            description: format!("spec {} is status Completed", id),
            verify: format!("`aida show {}` reports `Status: Completed`", id),
            kind: ClauseKind::Spec,
            arg: id.to_string(),
        });
    }
    if let Some(n) = pr {
        let noun = forge.change_noun();
        let verify = forge
            .change_cmd_hint("view", &format!("{n} --json state --jq .state"))
            .map(|cmd| format!("`{cmd}` prints `MERGED`"))
            .unwrap_or_else(|| format!("the {noun} #{n} is merged in the forge"));
        clauses.push(GoalClause {
            description: format!("{noun} #{n} is merged"),
            verify,
            kind: ClauseKind::Pr,
            arg: n.to_string(),
        });
    }
    if let Some(role) = queue_empty {
        let role = role.strip_prefix("role:").unwrap_or(role);
        clauses.push(GoalClause {
            description: format!("the {} queue is empty", role),
            verify: format!(
                "`aida queue list --for {} --no-in-flight` shows no queued items",
                role
            ),
            kind: ClauseKind::QueueEmpty,
            arg: role.to_string(),
        });
    }

    if clauses.is_empty() {
        anyhow::bail!(
            "no condition flags given — pass at least one of \
             --batch / --epic / --spec / --pr / --queue-empty"
        );
    }
    Ok(clauses)
}

/// Join clauses into a single `/goal`-pasteable condition string. Each
/// clause inlines its verification command so a Haiku-class evaluator
/// can check it deterministically.
// trace:TASK-242 | ai:claude
pub(crate) fn assemble_goal_condition(clauses: &[GoalClause]) -> String {
    clauses
        .iter()
        .map(|c| format!("{} (verify: {})", c.description, c.verify))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// `aida goal` handler — emit the condition framed, bare (`--invoke`),
/// or to the clipboard (`--copy`).
// trace:TASK-242 | ai:claude
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_goal_command(
    batch: Option<&str>,
    epic: Option<&str>,
    spec: Option<&str>,
    pr: Option<u64>,
    queue_empty: Option<&str>,
    copy: bool,
    invoke: bool,
    as_deep_link: bool,
) -> Result<()> {
    let forge = crate::forge::resolve_forge_kind(
        &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    );
    let clauses = build_goal_clauses_for_forge(batch, epic, spec, pr, queue_empty, forge)?;
    let condition = assemble_goal_condition(&clauses);
    let goal_line = format!("/goal {}", condition);

    // --invoke: bare line only, for `$(aida goal --invoke ...)`.
    if invoke {
        println!("{}", goal_line);
        return Ok(());
    }

    // SPIKE-33: emit a claude-cli:// deep link whose `q=` is the assembled
    // /goal line. Opens Claude Code in the current dir with the prompt
    // pre-filled (inert until Enter). trace:SPIKE-33 | ai:claude
    if as_deep_link {
        let cwd = std::env::current_dir().ok();
        let mut link = deep_link::DeepLink::new().with_prompt(&goal_line);
        if let Some(c) = cwd {
            link = link.with_cwd(c);
        }
        let rendered = link.render();
        if rendered.exceeds {
            println!(
                "{} the assembled /goal line exceeds Claude Code's 5000-char URL ceiling — \
                 emitting anyway, but the link may fail to open",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
        println!("{}", rendered.url);
        return Ok(());
    }

    if copy {
        if copy_to_clipboard(&goal_line) {
            println!(
                "{} copied the /goal condition to the clipboard",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            println!(
                "{} no clipboard tool found (wl-copy/xclip/xsel/pbcopy/clip) — printing instead",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
        println!();
        println!("  {}", goal_line.cyan());
        return Ok(());
    }

    // Default: framed output with the per-clause verification recipe.
    println!(
        "{} machine-checkable completion condition ({} clause{}):",
        crate::glyph(crate::glyphs::Glyph::FlowActive)
            .green()
            .bold(),
        clauses.len(),
        if clauses.len() == 1 { "" } else { "s" }
    );
    println!();
    println!("  {}", goal_line.cyan());
    println!();
    println!("  {}", "verify each clause:".dimmed());
    for c in &clauses {
        println!("    {} {}", "·".dimmed(), c.verify);
    }
    println!();
    println!(
        "  {}",
        "paste into /goal or /schedule — or re-run with --copy / --invoke".dimmed()
    );
    Ok(())
}

#[cfg(test)]
mod goal_command_tests {
    use super::{assemble_goal_condition, build_goal_clauses_for_forge};
    use crate::forge::ForgeKind;

    #[test]
    fn batch_clause_strips_prefix_and_emits_verify() {
        let c = build_goal_clauses_for_forge(
            Some("batch:plan-tooling"),
            None,
            None,
            None,
            None,
            ForgeKind::GitHub,
        )
        .unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0].description.contains("`batch:plan-tooling`"));
        assert!(c[0].verify.contains("aida list --tags batch:plan-tooling"));
        // Bare name (no prefix) behaves identically.
        let c2 = build_goal_clauses_for_forge(
            Some("plan-tooling"),
            None,
            None,
            None,
            None,
            ForgeKind::GitHub,
        )
        .unwrap();
        assert_eq!(c2[0].verify, c[0].verify);
    }

    #[test]
    fn epic_spec_pr_clauses() {
        let c = build_goal_clauses_for_forge(
            None,
            Some("EPIC-23"),
            None,
            None,
            None,
            ForgeKind::GitHub,
        )
        .unwrap();
        assert!(c[0].verify.contains("aida list --parent EPIC-23"));

        let c =
            build_goal_clauses_for_forge(None, None, Some("TASK-9"), None, None, ForgeKind::GitHub)
                .unwrap();
        assert!(c[0].verify.contains("aida show TASK-9"));

        let c = build_goal_clauses_for_forge(None, None, None, Some(42), None, ForgeKind::GitHub)
            .unwrap();
        assert!(c[0].verify.contains("gh pr view 42"));
    }

    #[test]
    fn pr_clause_uses_active_forge_vocabulary() {
        let github =
            build_goal_clauses_for_forge(None, None, None, Some(42), None, ForgeKind::GitHub)
                .unwrap();
        assert_eq!(github[0].description, "PR #42 is merged");
        assert!(github[0].verify.contains("gh pr view 42"));

        let gitlab =
            build_goal_clauses_for_forge(None, None, None, Some(42), None, ForgeKind::GitLab)
                .unwrap();
        assert_eq!(gitlab[0].description, "MR #42 is merged");
        assert!(gitlab[0].verify.contains("glab mr view 42"));
        assert!(!gitlab[0].verify.contains("gh pr"));
    }

    #[test]
    fn queue_empty_clause_strips_role_prefix() {
        let c = build_goal_clauses_for_forge(
            None,
            None,
            None,
            None,
            Some("role:implementer"),
            ForgeKind::GitHub,
        )
        .unwrap();
        assert!(c[0].verify.contains("aida queue list --for implementer"));
        let c2 = build_goal_clauses_for_forge(
            None,
            None,
            None,
            None,
            Some("implementer"),
            ForgeKind::GitHub,
        )
        .unwrap();
        assert_eq!(c2[0].verify, c[0].verify);
    }

    #[test]
    fn multiple_flags_compose_with_and() {
        let clauses = build_goal_clauses_for_forge(
            Some("lifecycle"),
            None,
            None,
            Some(30),
            None,
            ForgeKind::GitHub,
        )
        .unwrap();
        assert_eq!(clauses.len(), 2);
        let condition = assemble_goal_condition(&clauses);
        assert!(condition.contains(" AND "));
        assert!(condition.contains("batch:lifecycle"));
        assert!(condition.contains("PR #30"));
    }

    #[test]
    fn no_flags_is_an_error() {
        assert!(
            build_goal_clauses_for_forge(None, None, None, None, None, ForgeKind::GitHub).is_err()
        );
    }
}

/// Derive a machine-checkable completion condition from AIDA
/// metadata, ready to paste into `/goal` (or `/schedule`). Each flag
/// contributes one clause; multiple flags compose with `AND`. Every
/// clause carries an explicit verification command so a small
/// evaluator can check it deterministically — no vague "make the
/// tests pass" conditions that loop forever.
// trace:TASK-242 | ai:claude
///
/// `aida goal advisor …` keeps the operator's standing goal for the advisor
/// seat in the store (see `aida goal advisor --help`).
#[derive(clap::Parser, Debug)]
#[clap(name = "goal", args_conflicts_with_subcommands = true)]
pub(crate) struct GoalArgs {
    // trace:STORY-2243 | ai:claude
    #[clap(subcommand)]
    pub command: Option<GoalCommand>,

    /// Condition: all specs tagged `batch:<NAME>` reach a terminal
    /// status (Completed/Rejected). `batch:` prefix optional.
    #[clap(long, value_name = "NAME")]
    pub batch: Option<String>,

    /// Condition: all direct children of EPIC/STORY `<ID>` reach a
    /// terminal status.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude
    #[clap(long, value_name = "ID", value_parser = crate::spec_arg::spec_id)]
    pub epic: Option<String>,

    /// Condition: spec `<SPEC-ID>` reaches status Completed.
    #[clap(long, value_name = "SPEC-ID", value_parser = crate::spec_arg::spec_id)]
    pub spec: Option<String>,

    /// Condition: PR `<N>` is merged.
    #[clap(long, value_name = "N")]
    pub pr: Option<u64>,

    /// Condition: the given role's queue is empty. Accepts
    /// `implementer` or `role:implementer`.
    #[clap(long, value_name = "ROLE")]
    pub queue_empty: Option<String>,

    /// Copy the assembled `/goal` line to the system clipboard.
    #[clap(long, conflicts_with = "invoke")]
    pub copy: bool,

    /// Print only the bare `/goal <condition>` line, no framing —
    /// for scripting / command substitution.
    #[clap(long)]
    pub invoke: bool,

    /// Print a `claude-cli://open` deep link with the `/goal …` line
    /// pre-filled. Click → Claude Code opens in cwd with the prompt
    /// ready (inert until Enter). Requires Claude Code v2.1.91+.
    // trace:SPIKE-33 | ai:claude
    #[clap(long = "as-deep-link", conflicts_with_all = ["copy", "invoke"])]
    pub as_deep_link: bool,
}

/// `aida goal` subcommands.
// trace:STORY-2243 | ai:claude
#[derive(clap::Subcommand, Debug)]
pub(crate) enum GoalCommand {
    /// The operator's standing goal for the advisor seat, kept in the
    /// requirements store so a new or rotated advisor session (any vendor)
    /// reads the same goal. Only the operator sets it, from their own
    /// terminal; any seat can show, check and list it.
    Advisor {
        #[clap(subcommand)]
        action: AdvisorGoalAction,
    },
}

/// `aida goal advisor` actions.
// trace:STORY-2243 | ai:claude
#[derive(clap::Subcommand, Debug)]
pub(crate) enum AdvisorGoalAction {
    /// Set the advisor's goal: free text plus optional checkable clauses.
    /// Replaces the active goal; the previous one stays in history. Operator
    /// only, at an interactive terminal outside any agent session.
    Set {
        /// The goal, in your words.
        text: String,
        /// Clause: all specs tagged `batch:<NAME>` are resolved.
        #[clap(long, value_name = "NAME")]
        batch: Option<String>,
        /// Clause: all direct children of `<ID>` are resolved.
        #[clap(long, value_name = "ID", value_parser = crate::spec_arg::spec_id)]
        epic: Option<String>,
        /// Clause: spec `<SPEC-ID>` is Completed.
        #[clap(long, value_name = "SPEC-ID", value_parser = crate::spec_arg::spec_id)]
        spec: Option<String>,
        /// Clause: PR `<N>` is merged.
        #[clap(long, value_name = "N")]
        pr: Option<u64>,
        /// Clause: the given role's queue is empty.
        #[clap(long, value_name = "ROLE")]
        queue_empty: Option<String>,
    },
    /// Show the active goal: text, clauses with their verify commands, who
    /// set it and when.
    Show,
    /// Check each clause and print met/unmet. Exits 0 only when every clause
    /// is met; a goal with no checkable clauses is never met.
    Check,
    /// Every goal set, newest first.
    History,
}

// `aida goal` is a pure condition generator — no store needed.
// trace:TASK-242 | ai:claude
fn run_goal(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    use crate::*;
    let args = <GoalArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    // trace:STORY-2243 | ai:claude
    if let Some(GoalCommand::Advisor { action }) = &args.command {
        return goal_advisor::handle(action);
    }
    let GoalArgs {
        command: _,
        batch,
        epic,
        spec,
        pr,
        queue_empty,
        copy,
        invoke,
        as_deep_link,
    } = &args;
    return self::handle_goal_command(
        batch.as_deref(),
        epic.as_deref(),
        spec.as_deref(),
        *pr,
        queue_empty.as_deref(),
        *copy,
        *invoke,
        *as_deep_link,
    );
}

// trace:STORY-1728 | ai:claude
pub(crate) static GOAL: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "goal",
    help_after: "import-plan",
    command: <GoalArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Read,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Early(run_goal),
};

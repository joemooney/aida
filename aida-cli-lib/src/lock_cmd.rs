//! `aida lock` command cluster — the advisor-directed worktree advisory lock
//! (STORY-711 slice 1): acquire / verify / release / status of the
//! one-advisor-per-worktree gate.
//!
//! Dispatched before storage init — every subcommand reads/writes only
//! `.aida/sessions/*.toml` lease files; no requirement-store handle needed. The
//! locking machinery stays in `crate::worktree_lock` / `aida_core::lock`
//! (reached via `crate::` / `aida_core::`); only the command handler lives here.
//! Extracted verbatim from `main.rs` (SPIKE-78); no behavior change.

use anyhow::{Context, Result};
use colored::Colorize;

// trace:STORY-1728 | ai:claude
#[path = "lock_cmd_args.rs"]
pub(crate) mod lock_cmd_args;
pub(crate) use lock_cmd_args::*;

use crate::cli::LockCommand;
use crate::*;

// TASK-661 (ADR-3): per-scope disposition/triage lease. The authority gate
// (`has_advisor_authority`) decides WHO may dispose; this lease enforces HOW
// MANY — exactly one live advisor per scope. Acquire takes the lease (refused,
// naming the holder, if a live advisor already holds the scope), release frees
// it, status lists the live set (reaping dead holders on read). Reuses the
// same PID-liveness reaper primitive (`process_probe::pid_is_alive`) the
// session-lease reaper uses, so a crashed advisor cannot lock triage forever.
// trace:TASK-661 | ai:claude
/// Advisor-directed worktree lock (STORY-711 slice 1). Dispatched before
/// storage init, same reasoning as `handle_triage_command`: every subcommand
/// reads/writes only `.aida/sessions/*.toml` lease files — no requirement
/// store handle needed.
// trace:STORY-711 | ai:claude
pub(crate) fn handle_lock_command(cmd: &LockCommand) -> Result<()> {
    let project_root = find_project_root()?;
    match cmd {
        LockCommand::Acquire { worktree, advisor } => {
            let path = worktree_lock::acquire(&project_root, worktree, advisor)?;
            if agent_output_mode() {
                println!(
                    "{}",
                    toon::table_raw(
                        "lock",
                        &["worktree", "authorized_by", "lease"],
                        &[vec![
                            worktree.display().to_string(),
                            advisor.clone(),
                            path.display().to_string(),
                        ]],
                    )
                );
            } else {
                println!(
                    "{} worktree {} for advisor {} (lease: {}).",
                    "Locked".green().bold(),
                    worktree.display().to_string().cyan(),
                    advisor,
                    path.display(),
                );
            }
            Ok(())
        }
        LockCommand::Verify { worktree, r#as } => {
            let target = worktree
                .clone()
                .unwrap_or(std::env::current_dir().context("could not read current directory")?);
            let authorized_by = worktree_lock::read_authorized_by(&project_root, &target);
            let verdict =
                aida_core::lock::verify_worktree_lock(authorized_by.as_deref(), r#as.as_deref());
            match verdict {
                aida_core::lock::LockVerdict::Unlocked => {
                    if agent_output_mode() {
                        println!("{}", toon::scalar("verdict", "unlocked"));
                    } else {
                        println!(
                            "{} {} carries no advisor lock.",
                            "Unlocked:".green().bold(),
                            target.display()
                        );
                    }
                    Ok(())
                }
                aida_core::lock::LockVerdict::Authorized => {
                    if agent_output_mode() {
                        println!("{}", toon::scalar("verdict", "authorized"));
                    } else {
                        println!(
                            "{} {} is locked by your advisor ({}).",
                            "Authorized:".green().bold(),
                            target.display(),
                            r#as.as_deref().unwrap_or("")
                        );
                    }
                    Ok(())
                }
                aida_core::lock::LockVerdict::Refused { by } => {
                    // Non-zero exit so a caller (script or agent) can branch
                    // on it — the manual bouncer's whole point.
                    // trace:STORY-711 | ai:claude
                    if agent_output_mode() {
                        println!(
                            "{}",
                            toon::table_raw(
                                "verdict",
                                &["state", "by"],
                                &[vec!["refused".to_string(), by.clone(),]]
                            )
                        );
                        anyhow::bail!(
                            "worktree {} is locked by a different advisor ({by})",
                            target.display()
                        );
                    }
                    anyhow::bail!(
                        "{} {} is locked by advisor `{by}` — your token ({}) does not match. \
                         Coordinate with {by}, or have them run `aida lock release {}`.",
                        "Refused:".red().bold(),
                        target.display(),
                        r#as.as_deref().unwrap_or("<none>"),
                        target.display(),
                    );
                }
            }
        }
        LockCommand::Release { worktree } => {
            let released = worktree_lock::release(&project_root, worktree)?;
            if released {
                println!(
                    "{} the advisor lock on {}.",
                    "Released".green().bold(),
                    worktree.display()
                );
            } else {
                println!(
                    "{} {} carries no advisor lock.",
                    "Note:".yellow().bold(),
                    worktree.display()
                );
            }
            Ok(())
        }
        LockCommand::Status => {
            let locks = worktree_lock::list_locks(&project_root);
            if agent_output_mode() {
                let rows: Vec<Vec<String>> = locks
                    .iter()
                    .map(|l| {
                        vec![
                            l.worktree_path.clone(),
                            l.authorized_by.clone(),
                            l.scope.clone(),
                        ]
                    })
                    .collect();
                println!(
                    "{}",
                    toon::table_raw("locks", &["worktree", "authorized_by", "scope"], &rows)
                );
            } else if locks.is_empty() {
                println!("No worktrees are currently locked.");
            } else {
                println!("Locked worktrees:");
                for l in &locks {
                    println!("  {} — advisor {}", l.worktree_path.cyan(), l.authorized_by);
                }
            }
            Ok(())
        }
    }
}

/// Advisor-directed worktree lock — a MANUAL bouncer:
/// `acquire` stamps `authorized_by` on the session lease covering a
/// worktree, `verify` checks a token against it (exit 0/non-zero,
/// scriptable), `release` clears it, `status` lists locked worktrees.
/// Additive: nothing today calls `verify` automatically.
// trace:STORY-711 | ai:claude — spec id kept out of the user-facing about
// text (slice 1, generalizes BUG-637).
#[derive(clap::Parser, Debug)]
#[clap(name = "lock")]
pub(crate) struct LockArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::LockCommand,
}

// STORY-711 slice 1: `aida lock` reads/writes only `.aida/sessions/`
// lease files (the same registry `aida session leases` uses) — no
// requirement-store handle needed, same reasoning as `aida triage`
// above. trace:STORY-711 | ai:claude
fn run_lock(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <LockArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let lock_cmd = &args.inner;
    return self::handle_lock_command(lock_cmd);
}

// trace:STORY-1728 | ai:claude
pub(crate) static LOCK: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "lock",
    help_after: "terminal",
    command: <LockArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Early(run_lock),
};

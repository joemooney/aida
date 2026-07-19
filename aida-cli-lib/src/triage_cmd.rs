//! `aida triage` command cluster — the disposition/triage lease (TASK-661,
//! ADR-3): acquire / release / status of the one-disposing-advisor-per-scope
//! lease.
//!
//! Dispatched before storage init — every subcommand reads/writes only
//! `.aida/triage-leases/` files and probes PIDs; no requirement-store handle
//! needed. The shared lease machinery stays in `crate::triage_lease` (reached
//! via `crate::`); only the command handler lives here. Extracted verbatim from
//! `main.rs` (SPIKE-78); no behavior change.

use anyhow::Result;
use colored::Colorize;

use crate::cli::TriageCommand;
use crate::*;

pub(crate) fn handle_triage_command(cmd: &TriageCommand) -> Result<()> {
    let project_root = find_project_root()?;
    match cmd {
        TriageCommand::Acquire { scope, user } => {
            let slug = triage_lease::scope_slug(scope.as_deref());
            let owner = current_user_id(user.as_deref());
            // TASK-1150: distinct-user identity guard — minting a lease owned by
            // a genuinely-different user id than this shell's identity (e.g.
            // `--user user-b` from a `user-a` shell) silently crosses
            // identities. trace:TASK-1150 | ai:claude
            identity_guard::enforce(&current_user_id(None), &owner, "lease acquire")?;
            // Reap dead holders first so a crashed advisor's lease doesn't
            // wrongly block a fresh acquire. trace:TASK-661 | ai:claude
            let live =
                triage_lease::live_leases_reaping(&project_root, process_probe::pid_is_alive);
            // Record the disposing advisor's SHELL pid, not this short-lived
            // `aida` process — the `aida` wrapper is a shell function so our
            // parent IS the interactive shell / Claude session that's doing
            // the disposing. A process-id() lease would be reaped on the very
            // next invocation (the acquiring process has already exited),
            // never persisting; the shell pid lives as long as the advisor
            // session does, so the lease persists and is reaped only when the
            // advisor's shell dies. trace:TASK-661 | ai:claude
            let holder_pid = creator_shell_pid().unwrap_or_else(std::process::id);
            let new_lease = triage_lease::DispositionLease {
                scope: slug.clone(),
                owner: owner.clone(),
                pid: holder_pid,
                hostname: hostname(),
                started_at: chrono::Utc::now(),
            };
            match triage_lease::decide_acquire(new_lease, &live) {
                triage_lease::AcquireDecision::Granted(lease) => {
                    triage_lease::write_lease(&project_root, &lease)?;
                    println!(
                        "{} disposition lease for scope {} (held by {}).",
                        "Acquired".green().bold(),
                        slug.cyan(),
                        owner
                    );
                    println!(
                        "  Dispose from fresh substrate reads, then `aida triage release --scope {}`.",
                        slug
                    );
                    Ok(())
                }
                triage_lease::AcquireDecision::AlreadyHeld(_) => {
                    println!(
                        "{} you already hold the disposition lease for scope {}.",
                        "OK:".green().bold(),
                        slug.cyan()
                    );
                    Ok(())
                }
                triage_lease::AcquireDecision::Refused(holder) => {
                    // substrate-as-bouncer: a second disposing advisor is
                    // refused, naming the holder. Non-zero exit so scripts /
                    // drains can branch on it. trace:TASK-661 | ai:claude
                    anyhow::bail!(
                        "scope {} is already being disposed by {} (pid {} on {}, since {}). \
                         One disposing advisor per scope — coordinate with the holder or wait \
                         for them to release.",
                        slug,
                        holder.owner,
                        holder.pid,
                        holder.hostname,
                        holder.started_at.format("%Y-%m-%d %H:%M UTC"),
                    );
                }
            }
        }
        TriageCommand::Release { scope, user } => {
            let slug = triage_lease::scope_slug(scope.as_deref());
            let owner = current_user_id(user.as_deref());
            // TASK-1150: distinct-user identity guard. If a lease for this scope
            // is on disk owned by a genuinely-different identity than the one
            // we're releasing as (e.g. the shell drifted from `user-a` to
            // `user-b`), `release` would silently no-op ("no lease held by you")
            // while the real holder's lease sits untouched. Surface the
            // mismatch against the actual stored owner before that happens.
            // trace:TASK-1150 | ai:claude
            if let Some(held) = triage_lease::list_all(&project_root)
                .into_iter()
                .find(|l| l.scope == slug)
            {
                identity_guard::enforce(&owner, &held.owner, "lease release")?;
            }
            if triage_lease::release(&project_root, &slug, &owner)? {
                println!(
                    "{} disposition lease for scope {}.",
                    "Released".green().bold(),
                    slug.cyan()
                );
            } else {
                println!(
                    "{} no disposition lease held by {} for scope {}.",
                    "Note:".yellow().bold(),
                    owner,
                    slug.cyan()
                );
            }
            Ok(())
        }
        TriageCommand::Status { json } => {
            let live =
                triage_lease::live_leases_reaping(&project_root, process_probe::pid_is_alive);
            if *json {
                #[derive(serde::Serialize)]
                struct Row<'a> {
                    scope: &'a str,
                    owner: &'a str,
                    pid: u32,
                    hostname: &'a str,
                    started_at: String,
                }
                let rows: Vec<Row> = live
                    .iter()
                    .map(|l| Row {
                        scope: &l.scope,
                        owner: &l.owner,
                        pid: l.pid,
                        hostname: &l.hostname,
                        started_at: l.started_at.to_rfc3339(),
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else if live.is_empty() {
                println!("No active disposition leases. Any advisor may dispose.");
            } else {
                println!("Active disposition leases (one disposing advisor per scope):");
                for l in &live {
                    println!(
                        "  {} — {} (pid {} on {}, since {})",
                        l.scope.cyan(),
                        l.owner,
                        l.pid,
                        l.hostname,
                        l.started_at.format("%Y-%m-%d %H:%M UTC"),
                    );
                }
            }
            Ok(())
        }
    }
}

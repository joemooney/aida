// trace:TASK-3426 | ai:codex
use super::*;
use aida_core::git_ops::RemoteCredentialsExt as _;

/// STORY-384: `aida queue recover <id>` — the failed-phase-1 recovery wizard.
///
/// Inspects the spec's recovery-relevant state (lease, branch, worktree, PR),
/// derives the recommended recovery path via the pure
/// [`queue_recover::recommend`], prints the inspection + recommendation, and —
/// unless `--dry-run` — steps through the recovery, confirming destructive ops
/// (push, PR-create, session end) unless `--auto`.
///
/// A FRONT-END over existing primitives: the probes reuse the same lease/PR/git
/// helpers the orchestrator and `aida session leases` use; the execution shells
/// out to `aida` subcommands (`queue work --from-pr`, `pull`, `session end`,
/// `queue add`) and `git` / `gh` rather than reimplementing them.
/// trace:STORY-384 | ai:claude
/// `aida queue recover`'s push of the spec branch to origin. The branch can
/// come from a lease or the forge, so a dash-led name is refused and
/// options end before it. `Ok(false)` is a push git itself rejected.
// trace:BUG-1622 trace:BUG-1624 | ai:claude
pub(crate) fn recover_push_branch(repo: &std::path::Path, branch: &str) -> Result<bool> {
    crate::git_arg_guard::reject_option_like("branch", branch)?;
    let args = [
        "push",
        "-u",
        crate::git_arg_guard::END_OF_OPTIONS,
        "origin",
        branch,
    ];
    println!(
        "  {} git {}",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
        args.join(" ")
    );
    // The `-u` writes the shared config: clear a sandbox placeholder first
    // and repair the upstream if the write still failed.
    // trace:TASK-2305 | ai:claude
    aida_core::git_ops::clear_sandbox_config_lock(repo);
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        // trace:BUG-2181 | ai:claude
        .remote_credentials(repo, "origin")
        .status()?;
    if status.success() {
        aida_core::git_ops::repair_push_upstream(repo, "origin", Some(branch), branch);
    }
    Ok(status.success())
}

pub(crate) fn handle_queue_recover(
    storage: &Storage,
    user_id: &str,
    spec_query: &str,
    dry_run: bool,
    auto: bool,
) -> Result<()> {
    let project_root = find_project_root()?;

    // Resolve the spec id + status from the store (canonical SPEC-ID, status
    // for display). A spec we can't find still gets a best-effort recovery
    // using the query string verbatim, but we warn.
    let store = storage.load().ok();
    let (spec, status_label, spec_completed) = match store.as_ref().and_then(|s| {
        s.requirements.iter().find(|r| {
            r.spec_id.as_deref() == Some(spec_query)
                || r.agreed_id.as_deref() == Some(spec_query)
                || r.id.to_string() == spec_query
        })
    }) {
        Some(req) => {
            let id = req
                .agreed_id
                .clone()
                .or_else(|| req.spec_id.clone())
                .unwrap_or_else(|| spec_query.to_string());
            (
                id,
                req.status.to_string(),
                req.status == RequirementStatus::Completed,
            )
        }
        None => {
            eprintln!(
                "{} no spec matched `{}` in the store — probing git/PR state by the id verbatim.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                spec_query
            );
            (spec_query.to_string(), "<unknown>".to_string(), false)
        }
    };

    // --- Inspection: probe the world for this spec. ---
    // Reuse the orchestrator's PR/branch/CI/reviewed/merged probe (the same
    // helper `--from-pr` and `--resume-drain` use). `member: None` is the
    // standalone-PR case it already handles. trace:STORY-384 | ai:claude
    let (facts, pr_branch, pr) = probe_resume_facts(&project_root, storage, &spec, None);

    // Lease state — does a session still hold this spec's scope?
    let leases = list_leases(&project_root);
    let lease = find_lease_by_spec(&spec, &leases).ok();
    // trace:STORY-2325 | ai:codex — recovery never substitutes a PID/cwd scan.
    let lease_state = lease
        .as_ref()
        .map(|l| crate::ownership::reader::lease_owner(&project_root, l));

    // The branch we inspect for commits-ahead / dirty: prefer the lease's
    // branch (where a phase-1 implementer committed), else the PR head branch.
    let branch = lease
        .as_ref()
        .map(|l| l.branch.clone())
        .or_else(|| pr_branch.clone());

    // Commits ahead of origin/main on the branch (work that exists but may not
    // have shipped). Probe against the lease worktree if present, else root.
    let probe_repo = lease
        .as_ref()
        .map(|l| l.worktree_path.clone())
        .filter(|p| p.exists())
        .unwrap_or_else(|| project_root.clone());
    let commits_ahead = branch
        .as_deref()
        .and_then(|b| branch_commits_ahead_main(&probe_repo, b))
        .unwrap_or(0);

    // Branch pushed? A PR implies a pushed branch; otherwise probe origin.
    let branch_pushed = pr.is_some()
        || branch
            .as_deref()
            .map(|b| {
                matches!(
                    probe_branch_on_origin(&project_root, b),
                    BranchOriginProbe::Present
                )
            })
            .unwrap_or(false);

    // Worktree dirty? Only meaningful when a lease worktree exists.
    let dirty_entries = lease
        .as_ref()
        .filter(|l| l.worktree_path.exists())
        .map(|l| worktree_dirty_entries(&l.worktree_path))
        .unwrap_or_default();
    let uncommitted_changes = !dirty_entries.is_empty();

    let state = queue_recover::RecoverState {
        spec_completed: spec_completed || facts.spec_completed,
        pr_exists: pr.is_some(),
        pr_merged: facts.pr_merged,
        commits_ahead,
        branch_pushed,
        uncommitted_changes,
        lease_held: lease.is_some(),
    };

    // --- Print the inspection result. ---
    println!("🔎 recovery inspection for {}", spec.bold());
    println!("  {:<14} {}", "status".dimmed(), status_label);
    match (&lease, &lease_state) {
        (Some(l), Some(verdict)) => println!(
            "  lease {}: {}",
            l.id,
            crate::ownership::reader::owner_label(verdict)
        ),
        _ => println!("  lease none"),
    }
    match &branch {
        Some(b) => println!(
            "  {:<14} {} — {} commit(s) ahead of origin/main, {}",
            "branch".dimmed(),
            b,
            commits_ahead,
            if branch_pushed {
                "pushed"
            } else {
                "NOT pushed"
            }
        ),
        None => println!("  {:<14} none", "branch".dimmed()),
    }
    if uncommitted_changes {
        println!(
            "  {:<14} {} uncommitted change(s)",
            "worktree".dimmed(),
            dirty_entries.len()
        );
        for e in dirty_entries.iter().take(3) {
            println!("                   {}", e.dimmed());
        }
        if dirty_entries.len() > 3 {
            println!(
                "                   {} … and {} more",
                "".dimmed(),
                dirty_entries.len() - 3
            );
        }
    } else {
        println!("  {:<14} clean", "worktree".dimmed());
    }
    match pr {
        Some(n) if facts.pr_merged => println!("  {:<14} PR-{} (merged)", "pr".dimmed(), n),
        Some(n) => println!("  {:<14} PR-{} (open)", "pr".dimmed(), n),
        None => println!("  {:<14} none", "pr".dimmed()),
    }
    println!();

    // --- Recommendation. ---
    let action = queue_recover::recommend(&state);
    println!(
        "{} recommended: {}",
        "→".cyan().bold(),
        recover_action_label(action).bold()
    );
    println!("  {} {}", "rationale:".dimmed(), action.rationale());
    println!();

    if dry_run {
        println!(
            "{} --dry-run — not executing. Drop it to run the recovery.",
            crate::glyph(crate::glyphs::Glyph::Info).cyan()
        );
        return Ok(());
    }

    // --- Execution. ---
    // Refresh death proof only when the confirmed action ends the lease.
    // Pulling, pushing, parking WIP and driving an existing PR do not reclaim it.
    // trace:STORY-2325 | ai:codex
    let journal_reclaim = || -> Result<()> {
        if let Some(lease) = &lease {
            crate::ownership::reader::journal_reclaim(&project_root, lease).with_context(|| {
                format!(
                    "ownership reclaim could not end lease {}; inspect `aida session end {}`",
                    lease.id, lease.id
                )
            })?;
        }
        Ok(())
    };
    let aida = crate::aida_exe_path();
    // Helper: run an `aida` subcommand from the project root, surfacing its
    // exit code. Non-zero leaves the state visible for manual resume.
    let run_aida = |args: &[&str]| -> std::io::Result<std::process::ExitStatus> {
        println!(
            "  {} aida {}",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
            args.join(" ")
        );
        std::process::Command::new(&aida)
            .current_dir(&project_root)
            .args(args)
            .status_retrying_etxtbsy()
    };
    let run_git =
        |args: &[&str], cwd: &std::path::Path| -> std::io::Result<std::process::ExitStatus> {
            println!(
                "  {} git {}",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                args.join(" ")
            );
            std::process::Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .status()
        };
    // Confirm a destructive op unless --auto. Non-interactive (no TTY) defaults
    // to NO so a scripted run without --auto never silently ships.
    let confirm = |what: &str| -> bool {
        if auto {
            return true;
        }
        prompt_yes_no(&format!("  {} {} [y/N] ", "?".yellow(), what), false).unwrap_or(false)
    };

    match action {
        queue_recover::RecoverAction::AlreadyCompleted => {
            println!(
                "{} `{}` is already Completed — nothing to recover.",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                spec
            );
            if state.lease_held && confirm("end the leaked lease for this spec?") {
                journal_reclaim()?;
                let _ = run_aida(&["session", "end", &spec]);
            }
        }
        queue_recover::RecoverAction::AlreadyMergedPull => {
            println!(
                "{} the PR merged — pulling to auto-bump `{}` Done → Completed.",
                "↩".cyan().bold(),
                spec
            );
            let _ = run_aida(&["pull"]);
            if state.lease_held && confirm("end the leaked lease for this spec?") {
                journal_reclaim()?;
                let _ = run_aida(&["session", "end", &spec]);
            }
        }
        queue_recover::RecoverAction::DrivePhasesFromPr => {
            if !confirm("drive phases 3-6 on the open PR (reviewer → merge → pull → build)?")
            {
                println!(
                    "{} skipped — state left intact.",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan()
                );
                return Ok(());
            }
            // Reuse the TASK-405 PR-only orchestrator path.
            let _ = run_aida(&["queue", "work", &spec, "--auto-complete", "--from-pr"]);
        }
        queue_recover::RecoverAction::PushOpenPrDrive => {
            let Some(b) = branch.as_deref() else {
                eprintln!(
                    "{} no branch to push for `{}` — cannot recover automatically.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    spec
                );
                return Ok(());
            };
            if !confirm(&format!("push `{b}`, open a PR, then drive phases 3-6?")) {
                println!(
                    "{} skipped — state left intact.",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan()
                );
                return Ok(());
            }
            // trace:BUG-1622 trace:BUG-1624 | ai:claude
            if !recover_push_branch(&probe_repo, b)? {
                eprintln!(
                    "{} push failed — resolve manually, then re-run `aida queue recover {}`.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    spec
                );
                return Ok(());
            }
            // Open the change through the forge — title/body derived from the
            // branch commits (the forge-routed equivalent of `gh pr create
            // --fill`), so a GitLab/pure-git repo opens its own change.
            // trace:TASK-961 trace:STORY-621 | ai:claude
            if recover_open_change(&probe_repo, b) {
                // Now drive phases 3-6 on the freshly-opened change.
                let _ = run_aida(&["queue", "work", &spec, "--auto-complete", "--from-pr"]);
            } else {
                eprintln!(
                    "{} opening the {} failed — open it manually, then run \
                     `aida queue recover {}` again (it will take the drive-from-PR path).",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    crate::forge::resolve_forge_kind(&probe_repo).change_noun(),
                    spec
                );
            }
        }
        queue_recover::RecoverAction::WipCommitPushDrive => {
            if !confirm("commit the WIP, then push + open PR + drive phases 3-6?") {
                println!(
                    "{} skipped — commit/stash the WIP yourself, then re-run.",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan()
                );
                return Ok(());
            }
            let _ = run_git(&["add", "-A"], &probe_repo)?;
            let _ = run_git(
                &["commit", "-m", &format!("wip: recover {spec}")],
                &probe_repo,
            )?;
            let Some(b) = branch.as_deref() else {
                eprintln!(
                    "{} no branch to push.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
                );
                return Ok(());
            };
            // trace:BUG-1622 trace:BUG-1624 | ai:claude
            if !recover_push_branch(&probe_repo, b)? {
                eprintln!(
                    "{} push failed — resolve manually.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
                );
                return Ok(());
            }
            // Forge-routed change open — title/body derived from the branch
            // commits. trace:TASK-961 trace:STORY-621 | ai:claude
            if recover_open_change(&probe_repo, b) {
                let _ = run_aida(&["queue", "work", &spec, "--auto-complete", "--from-pr"]);
            } else {
                eprintln!(
                    "{} opening the {} failed — open it manually, then re-run recover.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    crate::forge::resolve_forge_kind(&probe_repo).change_noun()
                );
            }
        }
        queue_recover::RecoverAction::WipCommitPark => {
            println!(
                "{} preserving uncommitted work as a WIP commit (parked for resumption).",
                crate::glyph(crate::glyphs::Glyph::Info).cyan()
            );
            if !confirm("commit the WIP on the branch and park (no PR)?") {
                println!(
                    "{} skipped — state left intact.",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan()
                );
                return Ok(());
            }
            let _ = run_git(&["add", "-A"], &probe_repo)?;
            let _ = run_git(
                &["commit", "-m", &format!("wip: recover {spec} (parked)")],
                &probe_repo,
            )?;
            println!(
                "{} WIP committed. Resume later with `aida queue work {} --resume`.",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                spec
            );
        }
        queue_recover::RecoverAction::EndAndRequeue => {
            println!(
                "{} nothing was shipped — ending the lease and re-queueing `{}` for a fresh attempt.",
                "↩".cyan().bold(),
                spec
            );
            if !confirm("end the session and re-queue the spec?") {
                println!(
                    "{} skipped — state left intact.",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan()
                );
                return Ok(());
            }
            if state.lease_held {
                journal_reclaim()?;
                let _ = run_aida(&["session", "end", &spec]);
            }
            // Re-queue for the spec's owner. queue add is advisor-gated in
            // non-TTY contexts, so route the role explicitly.
            let _ = run_aida(&["queue", "add", &spec, "--user", user_id]);
        }
    }

    Ok(())
}

/// STORY-384: short human label for a recovery action (the recommendation
/// headline). The longer "why" is [`queue_recover::RecoverAction::rationale`].
/// trace:STORY-384 | ai:claude
pub(crate) fn recover_action_label(action: queue_recover::RecoverAction) -> &'static str {
    use queue_recover::RecoverAction as A;
    match action {
        A::AlreadyCompleted => "already completed — nothing to recover",
        A::AlreadyMergedPull => "pull to auto-bump the merged PR",
        A::DrivePhasesFromPr => "drive phases 3-6 on the open PR",
        A::PushOpenPrDrive => "push + open PR + drive phases 3-6",
        A::WipCommitPushDrive => "commit WIP + push + open PR + drive phases 3-6",
        A::WipCommitPark => "commit WIP and park for resumption",
        A::EndAndRequeue => "end the lease and re-queue",
    }
}

// trace:TASK-3426 | ai:codex
pub(super) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    _store_path: &std::path::Path,
) -> Result<()> {
    match cmd {
        // STORY-384: failed-phase-1 recovery wizard — inspect state, recommend a
        // recovery path, step through it. A front-end over existing primitives.
        // trace:STORY-384 | ai:claude
        QueueCommand::Recover {
            id,
            dry_run,
            auto,
            user,
        } => {
            let user_id = current_user_id(user.as_deref());
            handle_queue_recover(storage, &user_id, id, *dry_run, *auto)?;
        }
        _ => unreachable!("dispatcher selected another queue subcommand"),
    }
    Ok(())
}

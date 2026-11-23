use super::*;

/// STORY-720: options for the shared human-implementer finish ceremony.
// trace:STORY-720 | ai:claude — plain `//` keeps the marker out of any doc surface.
pub(crate) struct HumanFinishOptions {
    /// Explicit spec to finish; `None` ⇒ resolve from the branch / lease.
    pub(crate) spec: Option<String>,
    /// Commit subject when there is uncommitted work; `None` ⇒ a conventional
    /// default. The `(SPEC-ID)` trailer is always ensured.
    pub(crate) message: Option<String>,
    /// `--no-merge` — stop after opening the PR.
    pub(crate) no_merge: bool,
    /// `--no-pr` — stop after rebase + push.
    pub(crate) no_pr: bool,
    /// `--keep-worktree` — skip the worktree-cleanup step after merge.
    pub(crate) keep_worktree: bool,
    /// `--dry-run` — print the plan and exit.
    pub(crate) dry_run: bool,
    /// `--no-trailer-check` — forwarded to the finish tail's trailer guard.
    pub(crate) no_trailer_check: bool,
}

/// Resume this session's handed-off branch at its recorded PR head or a
/// detached descendant containing in-seat rework commits.
/// Git switch enforces worktree occupancy; unrelated detached checkouts refuse.
// trace:STORY-2325 | ai:codex
pub(crate) fn ship_branch(
    project: &std::path::Path,
    main: &std::path::Path,
    resume: bool,
) -> Result<String> {
    use crate::ownership::{store::Expect, Phase};
    let branch = current_git_branch(project)?;
    if !branch.is_empty() {
        return Ok(branch);
    }
    let inputs = aida_core::exec_context::Environment::capture();
    let session = inputs.var("AIDA_SESSION_ID").unwrap_or_default();
    let seat = project.canonicalize()?;
    let lease = list_leases(main)
        .into_iter()
        .find(|l| l.id == session && l.worktree_path.canonicalize().ok().as_ref() == Some(&seat))
        .ok_or_else(|| anyhow::anyhow!("detached HEAD has no owning session lease"))?;
    let store = crate::ownership::store::OwnershipStore::for_project(main)?
        .ok_or_else(|| anyhow::anyhow!("ownership store unavailable"))?;
    let record = crate::ownership::reader::lease_record(main, &lease)?
        .ok_or_else(|| anyhow::anyhow!("detached seat has no ownership handoff"))?;
    let Phase::InPr {
        head,
        handed_off_by: Some(by),
        ..
    } = &record.phase
    else {
        anyhow::bail!("detached seat has no recorded PR handoff");
    };
    anyhow::ensure!(
        by == &lease.id && record.holder.is_none(),
        "detached seat belongs to another session"
    );
    let tip = |reference: &str| -> Result<String> {
        let out = aida_core::git_ops::host_git_command(project)
            .args(["rev-parse", "--verify", reference])
            .output()?;
        anyhow::ensure!(out.status.success(), "cannot read handed-off branch tip");
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let branch_ref = format!("refs/heads/{}", lease.branch);
    let seat_head = tip("HEAD")?;
    anyhow::ensure!(
        tip(&branch_ref)? == *head,
        "handed-off branch moved beyond the recorded PR head; inspect the branch before resuming"
    );
    let ancestry = aida_core::git_ops::host_git_command(project)
        .args(["merge-base", "--is-ancestor", head, &seat_head])
        .output()?;
    anyhow::ensure!(
        ancestry.status.success(),
        "detached seat does not descend from the recorded PR head; inspect HEAD before resuming"
    );
    if !resume {
        if seat_head != *head {
            println!(
                "  dry-run: would fast-forward {branch_ref} from {head} to detached HEAD {seat_head} before pushing; the branch currently lacks the detached rework commits"
            );
        }
        return Ok(lease.branch);
    }
    let mut attached = false;
    let mut advanced = false;
    let result = store.update_all_checked(
        vec![crate::ownership::store::Step {
            key: record.key.clone(),
            op: "resume-ship".into(),
            expect: Expect::Generation(record.generation),
            transition: Box::new(|cur| {
                crate::ownership::ops::acquire(
                    cur,
                    &record.key,
                    crate::ownership::dual::holder_for_lease(
                        &lease,
                        &crate::ownership::dual::WriterFacts::here(),
                        chrono::Utc::now(),
                    ),
                    Phase::Leased,
                )
            }),
        }],
        || {
            anyhow::ensure!(
                tip("HEAD")? == seat_head && tip(&branch_ref)? == *head,
                "seat or branch moved before resume"
            );
            let worktrees = aida_core::git_ops::host_git_command(project)
                .args(["worktree", "list", "--porcelain"])
                .output()?;
            anyhow::ensure!(
                worktrees.status.success(),
                "cannot inspect branch occupancy"
            );
            let occupied = format!("branch {branch_ref}");
            anyhow::ensure!(
                !String::from_utf8_lossy(&worktrees.stdout)
                    .lines()
                    .any(|line| line == occupied),
                "handed-off branch is checked out in another worktree"
            );
            // Preserve detached rework with a compare-and-swap fast-forward.
            // trace:STORY-2325 | ai:codex
            if seat_head != *head {
                let out = aida_core::git_ops::host_git_command(project)
                    .args(["update-ref", &branch_ref, &seat_head, head])
                    .output()?;
                anyhow::ensure!(out.status.success(), "branch moved before rework recovery");
                advanced = true;
            }
            let out = aida_core::git_ops::host_git_command(project)
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "switch",
                    "--no-guess",
                    &lease.branch,
                ])
                .output()?;
            anyhow::ensure!(
                out.status.success(),
                "cannot resume handed-off branch: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            attached = true;
            Ok(())
        },
    );
    if result.is_err() && !store.dir().join(".pending.json").exists() {
        if attached {
            let out = aida_core::git_ops::host_git_command(project)
                .args(["update-ref", "--no-deref", "HEAD", &seat_head, &seat_head])
                .output()?;
            anyhow::ensure!(
                out.status.success(),
                "resume failed and HEAD restoration failed"
            );
        }
        if advanced {
            let out = aida_core::git_ops::host_git_command(project)
                .args(["update-ref", &branch_ref, head, &seat_head])
                .output()?;
            anyhow::ensure!(
                out.status.success(),
                "resume failed and branch restoration failed; inspect the branch before retrying"
            );
        }
    }
    result?;
    Ok(lease.branch)
}

/// STORY-720: the one-shot HUMAN-implementer finish — commit → rebase → push
/// → PR → CI → squash-merge → pull → worktree-cleanup.
///
/// This is the SHARED finish ceremony. `aida ship` is the direct caller today;
/// `aida zen` (STORY-721) and `aida integrate` (STORY-718) call the same fn so
/// the commit→rebase→PR→CI→merge→pull→cleanup sequence has exactly one home.
///
/// The finish TAIL (PR / CI / squash-merge / `aida pull` auto-bump /
/// worktree-cleanup) is NOT reimplemented — it delegates to
/// [`pr_ship_handler`] (TASK-458), the same machinery `aida pr ship` drives.
/// Only the human-implementer PREFIX is new here: commit the human's
/// uncommitted work with the `(SPEC-ID)` trailer, rebase onto current
/// `origin/main`, and push. Phase-1 (implement) is done by the human instead
/// of a spawned agent.
// trace:STORY-720 | ai:claude — plain `//` keeps the marker out of any doc surface.
pub(crate) fn run_human_finish_ceremony(opts: HumanFinishOptions) -> Result<()> {
    use ship::FinishMode;
    // `aida ship` merges unless told to stop at the PR; refuse that before
    // any commit or push. `--no-merge` / `--no-pr` stay open to a standalone
    // implementer. trace:STORY-1662 | ai:claude
    if !opts.no_merge && !opts.no_pr {
        crate::standalone_implementer::refuse_under_marker("ship")?;
    }

    let project_root = find_project_root()?;
    let main_worktree = main_worktree_root_from(&project_root);
    let branch = ship_branch(&project_root, &main_worktree, !opts.dry_run)?;
    if branch.is_empty() {
        anyhow::bail!(
            "could not detect the current branch (detached HEAD?) — `aida ship` runs from \
             inside the spec's worktree"
        );
    }
    if branch == "main" || branch == "master" {
        anyhow::bail!(
            "refusing to ship from `{branch}` — `aida ship` runs from inside the spec's \
             feature-branch worktree, not the default branch"
        );
    }

    // Resolve the spec: explicit arg → branch name → active session lease.
    let leases =
        crate::leases::list_leases_strict_all(&main_worktree).map_err(anyhow::Error::msg)?;
    let mut branch_leases = leases.iter().filter(|lease| lease.branch == branch);
    let lane = branch_leases.next();
    anyhow::ensure!(
        branch_leases.next().is_none(),
        "ambiguous lease for branch {branch}"
    );
    if let Some(lease) = lane {
        anyhow::ensure!(
            lease.worktree_path.canonicalize()? == project_root.canonicalize()?,
            "branch {branch} is leased to a different worktree"
        );
    }
    let lease_scope = lane.map(|lease| lease.scope.as_str());
    let spec = ship::resolve_spec(opts.spec.as_deref(), &branch, lease_scope).ok_or_else(|| {
        anyhow::anyhow!(
            "could not resolve a spec from branch `{branch}` — pass one explicitly: \
                 `aida ship <SPEC>`"
        )
    })?;

    let mode = ship::finish_mode(opts.no_pr, opts.no_merge);
    // Tracked/staged changes only: untracked paths (sandbox device mounts,
    // stray scratch files) never trigger or join the step-1 commit.
    // trace:TASK-2351 | ai:claude
    // trace:BUG-HOME-038 | ai:claude
    let dirty = ship::tracked_changes_pending(&project_root)?;

    // ---- Dry-run: print the resolved plan and exit. ----
    if opts.dry_run {
        print!("{}", ship::format_ship_plan(&spec, &branch, mode, dirty));
        return Ok(());
    }

    eprintln!(
        "{} aida ship — {} on branch {}",
        "→".cyan().bold(),
        // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
        aida_core::node_alias::display_spec_id(&spec).bold(),
        branch
    );

    // ---- Step 1: commit uncommitted work with the (SPEC-ID) trailer. ----
    if dirty {
        let subject = ship::commit_subject(&spec, opts.message.as_deref());
        eprintln!("  step 1: committing uncommitted work — {}", subject);
        // trace:BUG-b6b1278890774ef993d2ebc327862470-003 | ai:claude
        ship::refuse_staged_sandbox_placeholders(&project_root)?;
        ship::commit_tracked_changes(&project_root, &subject)?;
        eprintln!(
            "  {} committed",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        eprintln!("  step 1: no uncommitted work — nothing to commit");
    }
    if let Some(note) = ship::untracked_left_out_note(&ship::untracked_regular_files(&project_root))
    {
        eprintln!(
            "  {} {}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
            note
        );
    }

    // trace:BUG-2714 | ai:codex
    // ---- Step 2: rebase onto the branch's REAL base, not a hardcoded
    // origin/main (TASK-1416). A stacked PR (base = another in-flight
    // branch, not the repository default) that gets rebased onto main
    // anyway silently replays its parent's commits a second time — the
    // conflict this produces looks like a stale GitHub view, not the
    // local mistake it is. Resolve the base from the branch's open PR
    // (if any); fall back to the repository default when there's no PR
    // yet or its base can't be read, and SAY SO before acting (PRIN-5).
    let base_lookup = change_lookup_for_branch(&project_root, &branch);
    // Review fix: an unreachable forge (offline, gh failure, gh missing) is
    // "could not tell", not "no PR" — it must print the fallback note too.
    // trace:TASK-1416 | ai:claude
    let base_lookup_failed = finish_base_lookup_failed(&base_lookup);
    let pr_number_for_base = match base_lookup {
        crate::forge::ChangeLookup::Found(c) => Some(c.id),
        _ => None,
    };
    let pr_base_branch = pr_number_for_base.and_then(finish_ceremony_pr_base);
    let default_ref =
        detect_default_branch_ref(&project_root).unwrap_or_else(|| "origin/main".to_string());
    let rebase_target = pr_rebase::resolve_finish_rebase_base(
        pr_number_for_base,
        pr_base_branch.as_deref(),
        &default_ref,
    );
    if let Some(note) = rebase_target.note() {
        eprintln!(
            "  {} {}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
            note
        );
    } else if base_lookup_failed {
        eprintln!(
            "  {} could not determine this branch's PR base (the forge lookup failed); \
             falling back to {default_ref}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
        );
    }
    let origin_ref = rebase_target.origin_ref().to_string();
    integrate_and_push(
        &main_worktree,
        lane.map(|lease| lease.id.as_str()),
        &project_root,
        &branch,
        &origin_ref,
    )?;
    // D3 (BUG-2696): a push whose tip completes the spec marks it Done.
    // trace:BUG-2696 | ai:claude
    // F1: the hardened host git command; a failed read sets nothing (A2).
    let subject = crate::pr_merge_gate::branch_tip_subject(&project_root, "HEAD").ok();
    if let (Some(subject), Ok(tip)) = (subject, aida_core::git_ops::head_sha(&project_root)) {
        crate::publication_completion::mark_done_after_push_reported(
            &main_worktree,
            &spec,
            &subject,
            &tip,
        );
    }

    // ---- Finish: branch on the resolved mode. ----
    match mode {
        FinishMode::RebasePushOnly => {
            eprintln!(
                "{} aida ship — {} pushed (--no-pr; no PR opened)",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                aida_core::node_alias::display_spec_id(&spec).bold()
            );
            Ok(())
        }
        FinishMode::StopAtPr => {
            // Open the PR (or reuse an existing one for this branch), then stop.
            let pr = match change_lookup_for_branch(&project_root, &branch) {
                crate::forge::ChangeLookup::Found(c) => {
                    eprintln!("  step 4: found open PR-{} for branch {}", c.id, branch);
                    c.id
                }
                _ => {
                    // TASK-1442 (containment for BUG-1510): `spec` here is the
                    // already-resolved spec this ceremony is shipping for.
                    run_pr_open_spec_guard(&project_root, &branch, &spec);
                    let n = pr_ship_create_pr(&project_root, &branch)?;
                    eprintln!(
                        "  {} created PR-{}",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        n
                    );
                    n
                }
            };
            // BUG-2217: an open PR without a reviewer story never enters
            // review (PR-2895 stalled with "no queued review story"). File
            // it here, whatever the spec status; a skip says why and prints
            // the re-run command, and never fails the ship — the PR is open.
            // trace:BUG-2217 | ai:claude
            let _ = queue_review_for_open_pr(&project_root, &branch);
            // The owner detaches its seat and hands ownership to the PR.
            // trace:STORY-2325 | ai:codex
            crate::ownership::dual::handed_off(&project_root, &project_root, &branch, &spec, pr)?;
            eprintln!(
                "{} aida ship — PR-{} open for {} (--no-merge; not merged). Review + merge it, \
                 or finish with `aida ship`.",
                "⏸".yellow().bold(),
                pr,
                aida_core::node_alias::display_spec_id(&spec).bold()
            );
            Ok(())
        }
        FinishMode::FullShip => {
            // Reuse the existing finish machinery (TASK-458): pr_ship_handler
            // resolves / creates the PR, watches CI, squash-merges, runs
            // `aida pull` (Done → Completed auto-bump), and removes the
            // worktree. --keep-worktree maps to `pr ship`'s --no-cleanup.
            eprintln!("  step 4: finishing (CI → squash-merge → pull → cleanup)");
            pr_ship_handler(
                None,                  // resolve / create the PR for this branch
                false,                 // no_pull — run `aida pull`
                opts.keep_worktree,    // no_cleanup
                false,                 // dry_run
                false,                 // force_delete_branch
                None,                  // complexity
                None,                  // effort
                opts.no_trailer_check, // no_trailer_check
                false,                 // override_stale_check — not exposed on `aida ship` yet
                false, // override_stale_approval — not exposed on `aida ship` (TASK-1448)
                None,  // wait — not exposed on `aida ship`
                false, // no_complete — not exposed on `aida ship` (BUG-1919)
            )
        }
    }
}

/// True when the forge could not answer (as opposed to answering "no PR"):
/// the finish ceremony must then say it fell back rather than stay silent.
// trace:TASK-1416 | ai:claude
pub(crate) fn finish_base_lookup_failed(lookup: &crate::forge::ChangeLookup) -> bool {
    !matches!(
        lookup,
        crate::forge::ChangeLookup::Found(_) | crate::forge::ChangeLookup::NoChange
    )
}

#[cfg(test)]
mod task_1416_lookup_failure_tests {
    use super::finish_base_lookup_failed;
    use crate::forge::ChangeLookup;

    // trace:TASK-1416 | ai:claude
    #[test]
    fn unreachable_forge_is_a_lookup_failure_but_no_pr_is_not() {
        assert!(!finish_base_lookup_failed(&ChangeLookup::NoChange));
        assert!(finish_base_lookup_failed(&ChangeLookup::CliMissing));
        assert!(finish_base_lookup_failed(&ChangeLookup::CliFailed(
            "boom".into()
        )));
        assert!(finish_base_lookup_failed(&ChangeLookup::Unreachable(
            "offline".into()
        )));
    }
}

// trace:BUG-2714 | ai:codex
pub(crate) fn integrate_and_push(
    main: &std::path::Path,
    lane: Option<&str>,
    project_root: &std::path::Path,
    branch: &str,
    origin_ref: &str,
) -> Result<()> {
    let original = aida_core::git_ops::head_sha(project_root)?;
    let round = crate::ship_from_seat::publication_round(main, lane)?;
    let rework = round.is_some();
    let remote_branch = origin_ref
        .strip_prefix("origin/")
        .unwrap_or(origin_ref)
        .to_string();
    // The base comes from the forge (a PR's baseRefName); never let it
    // read as a git option. trace:BUG-1622 | ai:claude
    crate::git_arg_guard::reject_option_like("PR base branch", &remote_branch)?;
    let action = if rework { "merge" } else { "rebase" };
    eprintln!("  step 2: git {action} {origin_ref}");
    if let Some(round) = &round {
        anyhow::ensure!(
            round.base_branch == remote_branch,
            "PR base changed; resume rework pickup before shipping"
        );
    } else {
        let fetch = std::process::Command::new("git")
            .current_dir(project_root)
            .args([
                "fetch",
                crate::git_arg_guard::END_OF_OPTIONS,
                "origin",
                &remote_branch,
            ])
            // trace:BUG-2181 | ai:claude
            .remote_credentials(project_root, "origin")
            .status()
            .context("could not invoke `git fetch`")?;
        if !fetch.success() {
            anyhow::bail!("`git fetch origin {remote_branch}` failed — is the remote reachable?");
        }
    }
    let mut integrate = std::process::Command::new("git");
    integrate.current_dir(project_root).arg(action);
    if rework {
        // The tip subject carries the PR title and the completion trailer.
        let subject = crate::pr_merge_gate::branch_tip_subject(project_root, &original)?;
        integrate.args(["--no-edit", "-m", &subject]);
    }
    if !integrate
        .args([
            crate::git_arg_guard::END_OF_OPTIONS,
            round.as_ref().map_or(origin_ref, |r| r.base()),
        ])
        .status()?
        .success()
    {
        let _ = std::process::Command::new("git")
            .current_dir(project_root)
            .args([action, "--abort"])
            .status();
        anyhow::bail!(
            "git {action} onto {origin_ref} failed; inspect the worktree before retrying"
        );
    }

    // trace:BUG-2618 | ai:codex
    crate::pr_head_guard::check_branch(project_root, branch)?;
    // ---- Step 3: push (force-with-lease — the rebase may have rewritten history). ----
    // trace:TASK-2270 | ai:claude
    aida_core::git_ops::clear_sandbox_config_lock(project_root);
    let destination = if let Some(round) = round {
        let tip = aida_core::git_ops::head_sha(project_root)?;
        anyhow::ensure!(
            aida_core::git_ops::is_ancestor(project_root, &original, &tip)?,
            "ship tip lost the verified rework commits"
        );
        round.require_fix(project_root, &tip)?;
        format!("{tip}:refs/heads/{branch}")
    } else {
        branch.to_string()
    };
    eprintln!("  step 3: pushing {} to origin", branch);
    let push = std::process::Command::new("git")
        .current_dir(project_root)
        .arg("push")
        .args((!rework).then_some("--force-with-lease"))
        .args(["-u", "origin", &destination])
        // trace:BUG-2181 | ai:claude
        .remote_credentials(project_root, "origin")
        .status()
        .context("could not invoke `git push`")?;
    if !push.success() {
        anyhow::bail!("`git push to origin {branch}` failed — investigate before retrying");
    }
    aida_core::git_ops::repair_push_upstream(project_root, "origin", Some(branch), branch);
    eprintln!(
        "  {} pushed {}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        branch
    );
    Ok(())
}

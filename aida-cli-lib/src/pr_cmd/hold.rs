use super::*;

/// BUG-250: `aida pr hold` — deliberately hold the PR on the current session.
///
/// The implementer is intentionally not opening the PR yet (a manual gate
/// runs first). This resolves the session's spec + branch from the active
/// lease covering cwd, pushes the branch to origin and verifies the remote tip
/// (TASK-2793), and only then records a [`punt::HoldSignal`]. When
/// invoked under an `--auto-complete` drain the orchestrator provisions
/// [`punt::HOLD_SIGNAL_FILE_ENV`] pointing at an absolute path under the main
/// worktree root (the handshake is worktree-resolution-independent, exactly
/// like the punt signal); a standalone invocation still drops the marker under
/// the main root's `.aida/pr-holds/` so the state is recorded. Prints the hint
/// for opening the PR once the gate passes.
// trace:BUG-250 | ai:claude
pub(crate) fn pr_hold_handler(reason: Option<&str>) -> Result<()> {
    let project_root = find_main_worktree_root()?;
    let cwd = std::env::current_dir().context("could not resolve the current directory")?;
    let lease = active_lease_for_cwd(&project_root, &cwd).ok_or_else(|| {
        anyhow::anyhow!(
            "no active session lease covers this directory — `aida pr hold` runs inside \
             a `aida queue work` session worktree (it resolves the held spec + branch \
             from the session lease)."
        )
    })?;
    let spec = lease.scope.clone();
    let branch = lease.branch.clone();
    // TASK-2793: the hold claims the branch is on the forge, so push it and
    // confirm origin's tip matches before recording the hold or saying so.
    // trace:TASK-2793 | ai:claude
    let pushed_sha = push_and_verify_held_branch(&cwd, "origin", &branch)?;
    let signal = hold_signal(&project_root, &spec, &branch, reason);

    // Prefer the orchestrator-provisioned absolute path (handshake); fall back
    // to the conventional marker path under the main root for a standalone hold.
    let signal_path = std::env::var(punt::HOLD_SIGNAL_FILE_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| punt::hold_signal_path(&project_root, &spec));
    punt::write_hold_signal(&signal_path, &signal)
        .with_context(|| format!("could not write the PR-hold marker to {:?}", signal_path))?;

    eprintln!(
        "{} {} held — branch `{}` pushed to origin at {}, PR deliberately not opened{}",
        "⏸".yellow().bold(),
        // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
        aida_core::node_alias::display_spec_id(&spec).bold(),
        branch,
        &pushed_sha[..pushed_sha.len().min(12)],
        reason.map(|r| format!(" ({r})")).unwrap_or_default(),
    );
    // TASK-1240: derive the create-command + change noun from the resolved
    // forge so GitLab sessions see `glab mr create` and MR-N, not GitHub
    // wording. trace:TASK-1240 | ai:claude
    let forge = crate::forge::resolve_forge_kind(&project_root);
    let noun = forge.change_noun();
    let create_cmd = forge
        .create_cmd()
        .unwrap_or_else(|| "gh pr create".to_string());
    eprintln!(
        "  {} when your gate passes: `{}`, then `aida queue work {}-N --role reviewer`",
        "→".dimmed(),
        create_cmd,
        noun,
    );
    Ok(())
}

/// Push `branch` from `repo` to `remote` (setting upstream), then read the
/// remote's tip back with `ls-remote` and require it to equal the local
/// branch tip. Returns the verified sha. Any failure — the push is rejected,
/// the remote is unreachable, or the remote ref is absent or different —
/// is an error, so `aida pr hold` never reports a push it cannot prove.
// trace:TASK-2793 | ai:claude
pub(crate) fn push_and_verify_held_branch(
    repo: &std::path::Path,
    remote: &str,
    branch: &str,
) -> Result<String> {
    let full_ref = format!("refs/heads/{branch}");
    // Every git call goes through host_git_command: `repo` can be a seat
    // clone, where raw host git would run seat-written config and hooks
    // (e.g. pre-push). trace:BUG-2169 | ai:claude
    let local = aida_core::git_ops::host_git_command(repo)
        .args(["rev-parse", "--verify", "--quiet", &full_ref])
        .output()
        .context("could not invoke git rev-parse")?;
    if !local.status.success() {
        // Only rev-parse's quiet missing-ref result proves absence. In a
        // native seat, host_git_command can instead return the clone safety
        // refusal; retain that evidence without pushing or recording a hold.
        // trace:TASK-2852 | ai:codex
        let diagnostic = String::from_utf8_lossy(&local.stderr);
        let diagnostic = diagnostic.trim();
        if local.status.code() == Some(1) && diagnostic.is_empty() {
            anyhow::bail!("local branch `{branch}` does not exist — nothing to push for the hold");
        }
        anyhow::bail!(
            "could not resolve local branch `{branch}` in {} ({}) — the PR is not held{}",
            repo.display(),
            local.status,
            if diagnostic.is_empty() {
                String::new()
            } else {
                format!(": {diagnostic}")
            }
        );
    }
    let local_sha = String::from_utf8_lossy(&local.stdout).trim().to_string();

    // trace:BUG-2618 | ai:codex
    crate::pr_head_guard::check_branch(repo, branch)?;
    let push = aida_core::git_ops::host_git_command(repo)
        .args(["push", "-u", remote, &format!("{full_ref}:{full_ref}")])
        // trace:BUG-2181 | ai:claude
        .remote_credentials(repo, remote)
        .output()
        .context("could not invoke git push")?;
    // A push can upload the ref yet exit nonzero (e.g. `-u` cannot write the
    // upstream into a read-only `.git/config` in a sandbox), so origin's tip
    // decides success; the push's stderr only explains a failure.
    let push_err =
        (!push.status.success()).then(|| String::from_utf8_lossy(&push.stderr).trim().to_string());
    let not_pushed = |detail: &str| {
        anyhow::anyhow!(
            "could not push `{branch}` to {remote}, so the PR is not held: {detail}\n\
             Fix the push (e.g. `git push -u {remote} {branch}`), then re-run `aida pr hold`."
        )
    };

    let ls = aida_core::git_ops::host_git_command(repo)
        .args(["ls-remote", remote, &full_ref])
        // trace:BUG-2181 | ai:claude
        .remote_credentials(repo, remote)
        .output()
        .context("could not invoke git ls-remote")?;
    if !ls.status.success() {
        let ls_err = String::from_utf8_lossy(&ls.stderr).trim().to_string();
        return Err(match push_err {
            Some(e) => not_pushed(&e),
            None => anyhow::anyhow!(
                "pushed `{branch}` but could not verify it on {remote}, so the PR is not held: {ls_err}"
            ),
        });
    }
    let remote_tip =
        pr_rebase::parse_ls_remote_tip(&String::from_utf8_lossy(&ls.stdout), &full_ref);
    match (remote_tip, push_err) {
        (Some(tip), push_err) if tip == local_sha => {
            if let Some(e) = push_err {
                eprintln!(
                    "  {} {remote} has `{branch}` at {local_sha}, but git push also reported: {e}",
                    "⚠".yellow()
                );
            }
            Ok(local_sha)
        }
        (_, Some(e)) => Err(not_pushed(&e)),
        (Some(tip), None) => anyhow::bail!(
            "{remote} has `{branch}` at {tip}, not the local tip {local_sha} — the PR is not held"
        ),
        (None, None) => anyhow::bail!(
            "git push reported success but {remote} has no `{branch}` ref — the PR is not held"
        ),
    }
}

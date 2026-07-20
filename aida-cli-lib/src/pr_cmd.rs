//! `aida pr` command cluster — the pr dispatcher (`handle_pr_command`) and
//! its hold/rebase/ship/auto-queue-review handler layer, extracted from
//! `lib.rs` (SPIKE-78 / STORY-771; pure movement, no behavior change).
//! Shared PR-detection/git helpers stay in `lib.rs`, reached via `crate::`.
// trace:STORY-771 | ai:claude

use crate::*;

/// `aida pr <subcommand>` dispatcher. trace:STORY-90 | ai:claude
pub(crate) fn handle_pr_command(cmd: &PrCommand) -> Result<()> {
    match cmd {
        PrCommand::AutoQueueReview { branch } => pr_auto_queue_review(branch.as_deref()),
        PrCommand::Rebase {
            n,
            check,
            interactive,
            no_smoke,
            base,
            onto_parent,
        } => pr_rebase_handler(
            *n,
            *check,
            *interactive,
            *no_smoke,
            base.as_deref(),
            onto_parent.as_deref(),
        ),
        PrCommand::Ship {
            n,
            no_pull,
            no_cleanup,
            dry_run,
            force_delete_branch,
            complexity,
            effort,
            no_trailer_check,
        } => pr_ship_handler(
            *n,
            *no_pull,
            *no_cleanup,
            *dry_run,
            *force_delete_branch,
            *complexity,
            *effort,
            *no_trailer_check,
        ),
        PrCommand::Hold { reason } => pr_hold_handler(reason.as_deref()),
    }
}

/// BUG-250: `aida pr hold` — deliberately hold the PR on the current session.
///
/// The implementer pushed its branch but is intentionally not opening the PR
/// yet (a manual gate runs first). This resolves the session's spec + branch
/// from the active lease covering cwd and records a [`punt::HoldSignal`]. When
/// invoked under an `--auto-complete` drain the orchestrator provisions
/// [`punt::HOLD_SIGNAL_FILE_ENV`] pointing at an absolute path under the main
/// worktree root (the handshake is worktree-resolution-independent, exactly
/// like the punt signal); a standalone invocation still drops the marker under
/// the main root's `.aida/pr-holds/` so the state is recorded. Prints the hint
/// for opening the PR once the gate passes. trace:BUG-250 | ai:claude
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
    let signal = punt::HoldSignal {
        spec: spec.clone(),
        branch: branch.clone(),
        reason: reason.map(str::to_string),
    };

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
        "{} {} held — branch `{}` pushed, PR deliberately not opened{}",
        "⏸".yellow().bold(),
        spec.bold(),
        branch,
        reason.map(|r| format!(" ({r})")).unwrap_or_default(),
    );
    eprintln!(
        "  {} when your gate passes: `gh pr create` (or `glab mr create`), then \
         `aida queue work PR-N --role reviewer`",
        "→".dimmed()
    );
    Ok(())
}

/// Mode selector for `aida pr rebase`. Computed once from the CLI
/// flags so the rest of the handler is a flat match on intent.
/// trace:TASK-308 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrRebaseMode {
    /// Default: auto-rebase if clean, abort + recipe on conflict.
    Default,
    /// `--check`: report-only.
    Check,
    /// `--interactive`: leave temp worktree on conflict for the user
    /// to resolve, then continue + push.
    Interactive,
}

/// BUG-289: build the `aida pr rebase` fetch-failure message. When git's
/// stderr shows the branch is checked out in a worktree (the pr-N reviewer
/// worktree), surface a clear, actionable hint — end the lease or remove the
/// worktree — instead of the misleading "is the PR number correct?" line.
/// trace:BUG-289 | ai:claude
pub(crate) fn pr_fetch_failure_message(stderr: &str, n: u64, pr_local_branch: &str) -> String {
    if stderr.contains("checked out at") || stderr.contains("refusing to fetch into branch") {
        format!(
            "a worktree already holds the `{pr_local_branch}` branch, so `git fetch` \
             into it is refused.\n  To recover: end that worktree's lease \
             (`aida session leases`, then `aida session end <id>`) or remove the \
             worktree (`git worktree remove <path>`), then re-run `aida pr rebase {n}`.\n  \
             git said: {}",
            stderr.trim()
        )
    } else {
        format!(
            "could not fetch PR-{n}'s head ref (`refs/pull/{n}/head`) — \
             is the PR number correct and the remote reachable?"
        )
    }
}

/// `aida pr rebase <N>` — orchestrates the temp-worktree / fetch /
/// rebase / smoke / force-push-with-lease / cleanup recipe.
///
/// trace:TASK-308 | ai:claude
pub(crate) fn pr_rebase_handler(
    n: u64,
    check: bool,
    interactive: bool,
    no_smoke: bool,
    base_override: Option<&str>,
    // TASK-1080: stack-aware form. When set, step 6 runs the 3-arg
    // `git rebase --onto origin/<base> <onto_parent>` so the stacked parent's
    // (now squash-merged) commits are skipped, guarded by an ancestor check.
    onto_parent: Option<&str>,
) -> Result<()> {
    use pr_rebase::{
        cross_fork_refusal, default_smoke_check, manual_recipe, read_pr_rebase_config,
        resolve_smoke_check, temp_worktree_path,
    };

    let mode = if check {
        PrRebaseMode::Check
    } else if interactive {
        PrRebaseMode::Interactive
    } else {
        PrRebaseMode::Default
    };

    let project_root = find_project_root()?;

    // ---- Step 1: resolve PR metadata via the forge (STORY-621 Slice 2:
    // was a raw `gh pr view`). trace:TASK-963 | ai:claude ----
    let info = fetch_change_info_via_forge(&project_root, n)?;

    // ---- Step 2: refuse cross-fork PRs. ----
    if info.is_cross_repository {
        let msg = cross_fork_refusal(n, info.head_repo_owner.as_deref());
        eprintln!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
            msg
        );
        anyhow::bail!("cross-fork PR refused");
    }

    let base_ref = base_override
        .map(|s| s.to_string())
        .unwrap_or_else(|| info.base_ref.clone());
    let origin_base = format!("origin/{}", base_ref);

    // ---- Step 3: fetch origin so origin/<base> + the PR's head are
    // current. Required for both modes — --check reads ahead/behind
    // off origin/<base>; default mode rebases onto it. The fetch is
    // run in the *main* repo (refs are shared with the temp worktree
    // we create later), so the temp worktree starts with fresh refs.
    let fetch = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args(["fetch", "origin", "--prune"])
        .status();
    match fetch {
        Ok(s) if s.success() => {}
        Ok(_) => {
            eprintln!(
                "{} `git fetch origin` failed — refs may be stale",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
        Err(e) => {
            eprintln!(
                "{} could not invoke git fetch: {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                e
            );
        }
    }

    // Also fetch the PR's head ref explicitly, so `pr-<n>` exists
    // locally even when the PR is on a branch the local config
    // doesn't normally fetch. Mirrors how `aida session start --pr`
    // primes the worktree branch in EPIC-20.
    let pr_local_branch = format!("pr-{}", n);
    let pr_refspec = format!("+refs/pull/{n}/head:refs/heads/{pr_local_branch}");
    // BUG-289: capture stderr so the failure hint can branch on the actual
    // git error — git refuses to fetch into a local branch that's checked out
    // in a worktree (typically the pr-N reviewer worktree), which is a
    // different problem than a bad PR number / unreachable remote.
    let pr_fetch = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args(["fetch", "origin", pr_refspec.as_str()])
        .output();
    let fetch_ok = matches!(&pr_fetch, Ok(o) if o.status.success());
    if !fetch_ok {
        let stderr = pr_fetch
            .as_ref()
            .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
            .unwrap_or_default();
        anyhow::bail!(pr_fetch_failure_message(&stderr, n, &pr_local_branch));
    }

    // ---- Step 4 (--check only): report + exit zero. ----
    if matches!(mode, PrRebaseMode::Check) {
        return pr_rebase_check_report(&project_root, &info, &origin_base, &pr_local_branch);
    }

    // ---- Step 5: create the temp worktree on the PR branch. ----
    let wt_path = temp_worktree_path(&project_root, n);
    if wt_path.exists() {
        // Leftover from a previous interrupted run. Remove --force so
        // we don't strand the next attempt.
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args([
                "worktree",
                "remove",
                "--force",
                wt_path.to_str().unwrap_or(""),
            ])
            .status();
        let _ = std::fs::remove_dir_all(&wt_path);
    }
    if let Some(parent) = wt_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let wt_add = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args([
            "worktree",
            "add",
            wt_path.to_str().unwrap(),
            pr_local_branch.as_str(),
        ])
        .status();
    if !matches!(wt_add, Ok(s) if s.success()) {
        anyhow::bail!(
            "`git worktree add {} {}` failed — is the branch already \
             checked out in another worktree?",
            wt_path.display(),
            pr_local_branch
        );
    }

    let cleanup_worktree = || {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&project_root)
            .args([
                "worktree",
                "remove",
                "--force",
                wt_path.to_str().unwrap_or(""),
            ])
            .status();
        let _ = std::fs::remove_dir_all(&wt_path);
    };

    // ---- Step 5b (TASK-1080): stale-stack-record guard for --onto-parent. ----
    // The 3-arg rebase only makes sense when the recorded fork-point SHA is an
    // ancestor of the PR head; if it isn't, the branch was already rebased (the
    // STORY-248 cascade, a manual /aida-rebase, a sibling promotion) and
    // replaying `<sha>..HEAD` would pick the wrong commit range. Fail closed
    // with a pointer at the plain rebase rather than rewrite history off a
    // stale record. trace:TASK-1080 | ai:claude
    if let Some(parent_sha) = onto_parent {
        let is_ancestor = std::process::Command::new("git")
            .arg("-C")
            .arg(&wt_path)
            .args(["merge-base", "--is-ancestor", parent_sha, "HEAD"])
            .status();
        if !matches!(is_ancestor, Ok(s) if s.success()) {
            cleanup_worktree();
            anyhow::bail!(
                "--onto-parent {} is not an ancestor of the PR head — the stack record is \
                 stale (branch already rebased?). Re-check with `aida stack show`, or use a \
                 plain `aida pr rebase {}` if the branch no longer carries the parent's commits.",
                parent_sha,
                n
            );
        }
    }

    // ---- Step 6: rebase. ----
    // Plain form rebases everything onto origin/<base>; the --onto-parent form
    // uses `git rebase --onto origin/<base> <fork-sha>` so a stacked child
    // replays only its OWN commits (the parent's pre-squash commits, already on
    // main via the squash merge, are skipped). trace:TASK-1080 | ai:claude
    let rebase = {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(&wt_path);
        match onto_parent {
            Some(parent_sha) => cmd.args(["rebase", "--onto", &origin_base, parent_sha]),
            None => cmd.args(["rebase", &origin_base]),
        };
        cmd.status()
    };
    let rebase_ok = matches!(rebase, Ok(s) if s.success());

    if !rebase_ok {
        // Collect conflicting paths before aborting (default mode) or
        // before handing control to the user (interactive).
        let conflicts: Vec<String> = std::process::Command::new("git")
            .arg("-C")
            .arg(&wt_path)
            .args(["diff", "--name-only", "--diff-filter=U"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|l| l.to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        if matches!(mode, PrRebaseMode::Interactive) {
            eprintln!(
                "{} rebase hit {} conflict(s) in {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                conflicts.len(),
                wt_path.display()
            );
            for f in conflicts.iter().take(20) {
                eprintln!("    {}", f.dimmed());
            }
            eprintln!();
            eprintln!("Resolve in the worktree, then:");
            eprintln!("  cd {} && git rebase --continue", wt_path.display());
            eprintln!();
            eprintln!(
                "When done, press Enter to let aida force-push and clean up. \
                 Ctrl-C to bail (leaves the worktree in place)."
            );
            use std::io::BufRead;
            let mut line = String::new();
            let _ = std::io::stdin().lock().read_line(&mut line);

            // Verify the rebase actually finished — `.git/rebase-merge`
            // or `rebase-apply` still present ⇒ user pressed Enter
            // without resolving.
            let still_rebasing = wt_path.join(".git").join("rebase-merge").exists()
                || wt_path.join(".git").join("rebase-apply").exists();
            // .git in a linked worktree is a file pointing at the
            // gitdir; the rebase-merge marker actually lives at the
            // gitdir. Use `git status --porcelain=v2 --branch` to ask
            // git itself.
            let status_says_rebasing = std::process::Command::new("git")
                .arg("-C")
                .arg(&wt_path)
                .args(["status", "--porcelain=v2", "--branch"])
                .output()
                .ok()
                .map(|o| {
                    let s = String::from_utf8_lossy(&o.stdout).to_string();
                    s.contains("# branch.head (no branch)")
                        || s.lines().any(|l| l.starts_with("u "))
                })
                .unwrap_or(false);
            if still_rebasing || status_says_rebasing {
                eprintln!(
                    "{} rebase still in progress — leaving worktree at {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    wt_path.display()
                );
                anyhow::bail!("rebase not complete");
            }
            // Resolved — fall through to smoke check + push below.
        } else {
            // Default mode: abort, clean, print recipe, exit non-zero.
            let _ = std::process::Command::new("git")
                .arg("-C")
                .arg(&wt_path)
                .args(["rebase", "--abort"])
                .status();
            eprintln!(
                "{} rebase hit {} conflict(s) — aborted, worktree cleaned",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                conflicts.len()
            );
            for f in conflicts.iter().take(20) {
                eprintln!("    {}", f.dimmed());
            }
            eprintln!();
            eprintln!("{}", manual_recipe(n, &base_ref));
            cleanup_worktree();
            anyhow::bail!("rebase aborted due to conflicts");
        }
    }

    // ---- Step 7: smoke check. ----
    // TASK-969 trust boundary: smoke_check is executed via `sh -c` below, so
    // read_pr_rebase_config takes it from the TRUSTED default-branch copy of
    // .aida/config.toml (origin/<default>), not the branch-local working copy
    // a pushed branch controls. Fail-closed to the built-in default.
    let cfg = read_pr_rebase_config(&project_root);
    let project_default = default_smoke_check(&project_root);
    let smoke_cmd = resolve_smoke_check(no_smoke, &cfg, &project_default);

    if let Some(cmd) = smoke_cmd.as_deref() {
        eprintln!("{} smoke check: {}", "→".cyan(), cmd.dimmed());
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&wt_path)
            .status();
        let smoke_ok = matches!(status, Ok(s) if s.success());
        if !smoke_ok {
            eprintln!(
                "{} smoke check `{}` failed — leaving worktree at {} for inspection",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                cmd,
                wt_path.display()
            );
            anyhow::bail!("smoke check failed");
        }
    }

    // ---- Step 8 (BUG-640): patch-id force-push guard. ----
    // `--force-with-lease` does NOT protect a commit we already fetched,
    // so before overwriting the remote head we ask the LIVE remote what
    // it holds and refuse if any of those commits aren't incorporated
    // into our branch by patch-id. Fails CLOSED: any inability to verify
    // refuses the push. trace:BUG-640 | ai:claude
    //
    // Nested so the shell-out wrapper stays next to its one call site;
    // the pure pieces it composes (parse_ls_remote_tip, classify_force_push)
    // live in pr_rebase.rs and are unit-tested without git/network.
    fn force_push_guard(
        wt_path: &std::path::Path,
        remote: &str,
        head_ref: &str,
        base_ref: &str,
    ) -> pr_rebase::ForcePushGuard {
        let full_ref = format!("refs/heads/{head_ref}");
        // 1. Live remote tip for the target ref.
        let ls = std::process::Command::new("git")
            .arg("-C")
            .arg(wt_path)
            .args(["ls-remote", remote, &full_ref])
            .output();
        let ls = match ls {
            Ok(o) if o.status.success() => o,
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                return pr_rebase::ForcePushGuard::Inconclusive {
                    reason: format!(
                        "git ls-remote {remote} {full_ref} failed: {}",
                        stderr.trim()
                    ),
                };
            }
            Err(e) => {
                return pr_rebase::ForcePushGuard::Inconclusive {
                    reason: format!("could not invoke git ls-remote: {e}"),
                };
            }
        };
        let stdout = String::from_utf8_lossy(&ls.stdout);
        let tip = match pr_rebase::parse_ls_remote_tip(&stdout, &full_ref) {
            Some(t) => t,
            // Ref absent on the remote → nothing to overwrite → safe.
            None => return pr_rebase::ForcePushGuard::Safe,
        };
        // 2. Incorporation check: which commits does the remote tip have
        //    that our HEAD lacks, that aren't patch-id-equivalent to one
        //    of ours and aren't reachable from base?
        let range = format!("HEAD...{tip}");
        let not_base = format!("^{base_ref}");
        let revs = std::process::Command::new("git")
            .arg("-C")
            .arg(wt_path)
            .args([
                "rev-list",
                "--cherry-pick",
                "--right-only",
                "--oneline",
                &range,
                &not_base,
            ])
            .output();
        match revs {
            Ok(o) if o.status.success() => {
                pr_rebase::classify_force_push(&String::from_utf8_lossy(&o.stdout))
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                pr_rebase::ForcePushGuard::Inconclusive {
                    reason: format!("git rev-list incorporation check failed: {}", stderr.trim()),
                }
            }
            Err(e) => pr_rebase::ForcePushGuard::Inconclusive {
                reason: format!("could not invoke git rev-list: {e}"),
            },
        }
    }

    match force_push_guard(&wt_path, "origin", &info.head_ref, &origin_base) {
        pr_rebase::ForcePushGuard::Safe => {}
        pr_rebase::ForcePushGuard::Unincorporated { commits } => {
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                pr_rebase::force_push_block_message(&info.head_ref, &commits)
            );
            eprintln!(
                "  {}",
                format!("Worktree left at {} for inspection.", wt_path.display()).dimmed()
            );
            anyhow::bail!("force-push refused: remote has un-incorporated commits");
        }
        pr_rebase::ForcePushGuard::Inconclusive { reason } => {
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                pr_rebase::force_push_inconclusive_message(&info.head_ref, &reason)
            );
            eprintln!(
                "  {}",
                format!("Worktree left at {} for inspection.", wt_path.display()).dimmed()
            );
            anyhow::bail!("force-push refused: could not verify remote (fail-closed)");
        }
    }

    // ---- Step 8b: force-with-lease push. ----
    // `--force-with-lease=<refname>:<expected-sha>` anchors the lease
    // to the PR head we resolved up-front, so a concurrent push from
    // elsewhere makes this fail loudly instead of silently
    // overwriting. Belt + suspenders with the BUG-640 guard above.
    let lease = format!("{}:{}", info.head_ref, info.head_oid);
    let push_status = std::process::Command::new("git")
        .arg("-C")
        .arg(&wt_path)
        .args([
            "push",
            &format!("--force-with-lease={lease}"),
            "origin",
            &format!("HEAD:refs/heads/{}", info.head_ref),
        ])
        .status();
    let push_ok = matches!(push_status, Ok(s) if s.success());
    if !push_ok {
        eprintln!(
            "{} force-with-lease push failed — leaving worktree at {} for inspection",
            crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
            wt_path.display()
        );
        eprintln!(
            "  {}",
            "Likely cause: PR head moved on origin since we read it. \
             Pull the new tip and re-run."
                .dimmed()
        );
        anyhow::bail!("push failed");
    }

    // Report new SHA.
    let new_sha = std::process::Command::new("git")
        .arg("-C")
        .arg(&wt_path)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .chars()
                .take(12)
                .collect::<String>()
        })
        .unwrap_or_else(|| "(unknown)".to_string());
    eprintln!(
        "{} PR-{} rebased onto {} — new head {}",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        n,
        origin_base,
        new_sha
    );

    // ---- Step 9: cleanup. ----
    cleanup_worktree();

    Ok(())
}

/// STORY-621 Slice 2: fetch a change's scalar metadata through the forge and
/// adapt it to the `pr_rebase::PrInfo` shape the rebase + reviewer pre-flight
/// paths consume. Enforces the same required fields `pr_rebase::parse_pr_info`
/// does (base/head ref + head SHA are hard errors — the recipes can't proceed
/// without them) and carries a forge-aware "is the CLI installed?" context so
/// a GitLab user is never told to install `gh`.
// trace:TASK-963 | ai:claude
pub(crate) fn fetch_change_info_via_forge(
    project_root: &std::path::Path,
    n: u64,
) -> Result<pr_rebase::PrInfo> {
    let noun = crate::forge::resolve_forge_kind(project_root).change_noun();
    let cli = crate::forge::resolve_forge_kind(project_root).cli_name();
    let m = crate::forge::forge_for(project_root)
        .change_metadata(n, &mut network_retry::NoopSink)
        .with_context(|| {
            if cli.is_empty() {
                format!("could not resolve {noun} metadata — this project has no forge (pure-git)")
            } else {
                format!(
                    "could not resolve {noun} metadata — is `{cli}` installed and authenticated?"
                )
            }
        })?;
    anyhow::ensure!(
        !m.base_ref.is_empty(),
        "the forge did not return a base branch for {noun} {n}"
    );
    anyhow::ensure!(
        !m.head_ref.is_empty(),
        "the forge did not return a head branch for {noun} {n}"
    );
    anyhow::ensure!(
        !m.head_sha.is_empty(),
        "the forge did not return a head SHA for {noun} {n}"
    );
    Ok(pr_rebase::PrInfo {
        n,
        base_ref: m.base_ref,
        head_ref: m.head_ref,
        head_oid: m.head_sha,
        is_cross_repository: m.is_cross_repository,
        head_repo_owner: m.head_repo,
        is_draft: m.is_draft,
    })
}

/// Run `gh pr view <N> --json …` and return the parsed JSON. Pulled
/// out of `pr_rebase_handler` so the side-effecting call is isolated
/// (parse rules are pinned by `pr_rebase::parse_pr_info` tests).
/// trace:TASK-308 | ai:claude
pub(crate) fn fetch_pr_info_via_gh_bin(
    project_root: &std::path::Path,
    n: u64,
    gh_bin: &std::ffi::OsStr,
) -> Result<serde_json::Value> {
    let n_str = n.to_string();
    let out = std::process::Command::new(gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "view",
            n_str.as_str(),
            "--json",
            "baseRefName,headRefName,headRefOid,isCrossRepository,headRepository,isDraft",
        ])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!("`gh` not on PATH — install from https://cli.github.com/")
            } else {
                anyhow::anyhow!("`gh pr view {}` failed to spawn: {}", n, e)
            }
        })?;
    if !out.status.success() {
        anyhow::bail!(
            "`gh pr view {}` exited {} — {}",
            n,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("`gh pr view {}` returned non-JSON output", n))?;
    Ok(json)
}

/// `aida pr ship [<N>]` — collapse the "create-if-needed + watch CI +
/// squash-merge + pull + worktree-aware cleanup" recipe into one call.
/// The direct-publish counterpart to `aida queue work PR-N
/// --auto-complete` — used for human-pre-approved work where no
/// orchestrator review phase is needed.
///
/// All gh subprocess calls go through the BUG-286 retry wrapper so a
/// sub-second network blip during `gh pr merge` doesn't abort the
/// flow. The post-merge `aida pull` and `aida session end` are spawned
/// as `aida` subcommands (resolved via `current_exe()`) so the wrapper
/// inherits the existing pull / session-end implementations (the BUG-108
/// cwd warning, the auto-bump scan, the live-claude refusal) without
/// double-implementing them.
///
/// trace:TASK-458 | ai:claude
/// STORY-529/BUG-727: the ship-time spec-gate lookup — the spec-IDs
/// referenced by the commits about to ship, joined against the store as
/// `(id, tags, execution_mode)`. Reuses the trailer-gate machinery (range →
/// commits → store) so the ship gates see the same specs the trailer guard
/// validates. Empty when no store/commits resolve or nothing matches.
// trace:STORY-529 trace:BUG-727 | ai:claude
pub(crate) type ShipGateSpecRecord = (String, Vec<String>, Option<aida_core::ExecutionMode>);
pub(crate) fn ship_gate_spec_records(project_root: &std::path::Path) -> Vec<ShipGateSpecRecord> {
    let range = resolve_gate_range(project_root, None);
    let Ok(commits) = read_commits_in_range(project_root, &range) else {
        return Vec::new();
    };
    let Some(store) = load_store_for_lookup(project_root) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = Vec::new();
    for (_sha, subject) in &commits {
        ids.extend(pr_ship::extract_trailing_spec_ids_from_subject(subject));
    }
    ids.sort();
    ids.dedup();
    let mut out = Vec::new();
    for id in ids {
        let want = id.to_ascii_uppercase();
        let found = store.requirements.iter().find(|r| {
            r.spec_id
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
                || r.agreed_id
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case(&want))
                    .unwrap_or(false)
        });
        if let Some(req) = found {
            let tags: Vec<String> = req.tags.iter().cloned().collect();
            out.push((id, tags, req.execution_mode));
        }
    }
    out
}

/// The spec-IDs referenced by the commits about to ship whose spec carries
/// the `review:draft-only` tag — the ship-time draft gate's input.
// trace:STORY-529 | ai:claude
pub(crate) fn draft_only_specs_for_ship(project_root: &std::path::Path) -> Vec<String> {
    ship_gate_spec_records(project_root)
        .into_iter()
        .filter(|(_, tags, _)| pr_ship::is_draft_only_tagged(tags))
        .map(|(id, _, _)| id)
        .collect()
}

/// The specs behind the commits about to ship whose `execution_mode` holds
/// the auto-merge, as `(spec-id, mode-label)` pairs — the supervised-merge
/// gate's input. Empty when the merge may proceed (all specs are `drain`, or
/// the caller is an interactive human at a TTY).
// trace:BUG-727 | ai:claude
pub(crate) fn supervised_specs_for_ship(
    project_root: &std::path::Path,
    interactive_tty: bool,
) -> Vec<(String, String)> {
    let specs: Vec<(String, Option<aida_core::ExecutionMode>)> =
        ship_gate_spec_records(project_root)
            .into_iter()
            .map(|(id, _, mode)| (id, mode))
            .collect();
    pr_ship::supervised_merge_holds(&specs, interactive_tty)
}

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

    let project_root = find_project_root()?;
    let main_worktree = main_worktree_root_from(&project_root);
    let branch = current_git_branch(&project_root)?;
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
    let lease_scope = list_leases(&main_worktree)
        .into_iter()
        .find(|l| l.branch == branch)
        .map(|l| l.scope);
    let spec = ship::resolve_spec(opts.spec.as_deref(), &branch, lease_scope.as_deref())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "could not resolve a spec from branch `{branch}` — pass one explicitly: \
                 `aida ship <SPEC>`"
            )
        })?;

    let mode = ship::finish_mode(opts.no_pr, opts.no_merge);
    let dirty = working_tree_is_dirty(&project_root);

    // ---- Dry-run: print the resolved plan and exit. ----
    if opts.dry_run {
        print!("{}", ship::format_ship_plan(&spec, &branch, mode, dirty));
        return Ok(());
    }

    eprintln!(
        "{} aida ship — {} on branch {}",
        "→".cyan().bold(),
        spec.bold(),
        branch
    );

    // ---- Step 1: commit uncommitted work with the (SPEC-ID) trailer. ----
    if dirty {
        let subject = ship::commit_subject(&spec, opts.message.as_deref());
        eprintln!("  step 1: committing uncommitted work — {}", subject);
        let add = std::process::Command::new("git")
            .current_dir(&project_root)
            .args(["add", "-A"])
            .status()
            .context("could not invoke `git add`")?;
        if !add.success() {
            anyhow::bail!("`git add -A` failed — investigate before retrying");
        }
        let commit = std::process::Command::new("git")
            .current_dir(&project_root)
            .args(["commit", "-m", &subject])
            .status()
            .context("could not invoke `git commit`")?;
        if !commit.success() {
            anyhow::bail!("`git commit` failed — investigate before retrying");
        }
        eprintln!(
            "  {} committed",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        eprintln!("  step 1: no uncommitted work — nothing to commit");
    }

    // ---- Step 2: rebase onto current origin/main. ----
    eprintln!("  step 2: rebasing onto current origin/main");
    let fetch = std::process::Command::new("git")
        .current_dir(&project_root)
        .args(["fetch", "origin", "main"])
        .status()
        .context("could not invoke `git fetch`")?;
    if !fetch.success() {
        anyhow::bail!("`git fetch origin main` failed — is the remote reachable?");
    }
    let rebase = std::process::Command::new("git")
        .current_dir(&project_root)
        .args(["rebase", "origin/main"])
        .status()
        .context("could not invoke `git rebase`")?;
    if !rebase.success() {
        // Abort so the worktree is left clean, then bail with the manual recipe.
        let _ = std::process::Command::new("git")
            .current_dir(&project_root)
            .args(["rebase", "--abort"])
            .status();
        anyhow::bail!(
            "rebase onto origin/main hit conflicts — aborted, worktree left clean.\n  \
             Resolve by hand: `git rebase origin/main`, fix the conflicts, \
             `git rebase --continue`, then re-run `aida ship`."
        );
    }
    eprintln!(
        "  {} rebased onto origin/main",
        crate::glyph(crate::glyphs::Glyph::Check).green()
    );

    // ---- Step 3: push (force-with-lease — the rebase may have rewritten history). ----
    eprintln!("  step 3: pushing {} to origin", branch);
    let push = std::process::Command::new("git")
        .current_dir(&project_root)
        .args(["push", "--force-with-lease", "-u", "origin", &branch])
        .status()
        .context("could not invoke `git push`")?;
    if !push.success() {
        anyhow::bail!(
            "`git push --force-with-lease -u origin {branch}` failed — investigate before retrying"
        );
    }
    eprintln!(
        "  {} pushed {}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        branch
    );

    // ---- Finish: branch on the resolved mode. ----
    match mode {
        FinishMode::RebasePushOnly => {
            eprintln!(
                "{} aida ship — {} rebased + pushed (--no-pr; no PR opened)",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                spec.bold()
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
                    let n = pr_ship_create_pr(&project_root, &branch)?;
                    eprintln!(
                        "  {} created PR-{}",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        n
                    );
                    n
                }
            };
            eprintln!(
                "{} aida ship — PR-{} open for {} (--no-merge; not merged). Review + merge it, \
                 or finish with `aida ship`.",
                "⏸".yellow().bold(),
                pr,
                spec.bold()
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
            )
        }
    }
}

// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pr_ship_handler(
    n: Option<u64>,
    no_pull: bool,
    no_cleanup: bool,
    dry_run: bool,
    force_delete_branch: bool,
    complexity: Option<complexity_calibration::ComplexityLevel>,
    effort: Option<effort_calibration::EffortBucket>,
    no_trailer_check: bool,
) -> Result<()> {
    use pr_ship::{
        branch_pr_resolution_from_lookup, format_activity_event, format_dry_run_plan,
        recovery_hint, BranchPrResolution, PrShipOptions, ShipStep, StepOutcome,
    };

    let opts = PrShipOptions {
        pr_number: n,
        no_pull,
        no_cleanup,
        dry_run,
        complexity,
        effort,
    };

    // Project root for gh/git invocations is wherever the user is —
    // typical pattern: invoked from inside the spec worktree. The
    // main worktree (where `.aida-store` lives and `aida pull` must
    // run) may be a different path; we resolve it explicitly.
    let project_root = find_project_root()?;
    let main_worktree = main_worktree_root_from(&project_root);

    let branch = current_git_branch(&project_root)?;
    if branch.is_empty() {
        anyhow::bail!("could not detect current branch (detached HEAD?)");
    }

    // ---- STORY-469 Guard 1: validate trailer spec-IDs before shipping. ----
    // Catch a hallucinated / typo'd / since-rejected `(SPEC-ID)` trailer BEFORE
    // the commit reaches the PR + shared history. Reuses the STORY-498 gate's
    // pure validator + store resolver, client-side. Refuses (exit 1) on a dead
    // reference; `--no-trailer-check` and a store-less checkout both bypass.
    // trace:STORY-469 | ai:claude
    if !dry_run {
        run_client_trailer_guard(&project_root, "pr ship", no_trailer_check);
    }

    // ---- Step 1: resolve or create the PR. ----
    eprintln!(
        "{} aida pr ship — target branch: {}",
        "→".cyan().bold(),
        branch
    );

    // BUG-574: track whether the PR we resolve has ALREADY been merged. Re-running
    // `aida pr ship` on a spec that already shipped (the agent/loop retries, or a
    // sibling already merged it) is a benign no-op, not a failure: there is
    // nothing left to merge, so the merge step would otherwise error non-zero
    // (`gh pr merge` on a merged PR) and poison `pr ship`'s usage telemetry. When
    // already-merged, we skip CI-watch + merge and fall through to the idempotent
    // pull + cleanup steps, exiting 0. trace:BUG-574 | ai:claude
    let mut already_merged = false;
    let mut target_change: Option<crate::forge::ChangeRef> = None;

    let pr_number = match opts.pr_number {
        Some(explicit) => {
            eprintln!("  step 1: using explicit PR-{}", explicit);
            target_change = open_change_by_number(&project_root, explicit);
            // BUG-574: an explicit PR-N that is already merged is "already
            // shipped" — not a failure. Detect now so the merge step is skipped.
            if !dry_run {
                let mut probe_sink = crate::network_retry::NoopSink;
                if pr_is_merged_with_sink(&project_root, explicit as u32, &mut probe_sink)
                    == Some(true)
                {
                    already_merged = true;
                }
            }
            log_ship_activity(
                &main_worktree,
                Some(explicit),
                &ShipStep::ResolvePr {
                    create_if_needed: false,
                },
                &StepOutcome::Ok,
            );
            explicit
        }
        None => {
            // Look up an open change for the current branch through the forge
            // (GitHub: `gh pr list --head`; GitLab: `glab mr list --source-branch`).
            // TASK-141: only a definitive "no change" may fall through to
            // create. CLI/auth/network/parse failures are inconclusive; creating
            // after those failures masks the resume problem behind gh's "PR
            // already exists for this branch" error.
            // trace:TASK-961 trace:STORY-621 trace:TASK-141 | ai:codex
            let lookup = change_lookup_for_branch(&project_root, &branch);
            match branch_pr_resolution_from_lookup(&lookup) {
                BranchPrResolution::Found(existing) => {
                    // BUG-733: keep the found ChangeRef so the stacked-guard
                    // below resolves the PR's real head branch.
                    if let crate::forge::ChangeLookup::Found(c) = lookup {
                        target_change = Some(c);
                    }
                    eprintln!("  step 1: found open PR-{} for branch {}", existing, branch);
                    log_ship_activity(
                        &main_worktree,
                        Some(existing),
                        &ShipStep::ResolvePr {
                            create_if_needed: false,
                        },
                        &StepOutcome::Ok,
                    );
                    existing
                }
                BranchPrResolution::LookupFailed(reason) => {
                    log_ship_activity(
                        &main_worktree,
                        None,
                        &ShipStep::ResolvePr {
                            create_if_needed: false,
                        },
                        &StepOutcome::Failed(reason.clone()),
                    );
                    anyhow::bail!(reason);
                }
                BranchPrResolution::Create => {
                    if dry_run {
                        eprintln!(
                            "  step 1: would create new PR for branch {} (dry-run)",
                            branch
                        );
                        // Use a placeholder so the rest of the dry-run plan
                        // still has a coherent N to print.
                        0
                    } else if let Some(merged_n) =
                        latest_merged_pr_for_branch(&project_root, &branch)
                    {
                        // BUG-574: no OPEN PR for the branch, but a merged one exists —
                        // the branch already shipped. `pr_ship_create_pr` would fail with
                        // "no commits between" (a benign already-shipped state reported as
                        // a hard error). Treat it as already-merged and exit 0 after the
                        // idempotent pull/cleanup steps. trace:BUG-574 | ai:claude
                        eprintln!(
                            "  step 1: no open PR for branch {} — PR-{} already merged",
                            branch, merged_n
                        );
                        already_merged = true;
                        log_ship_activity(
                            &main_worktree,
                            Some(merged_n),
                            &ShipStep::ResolvePr {
                                create_if_needed: false,
                            },
                            &StepOutcome::Skipped("PR already merged for this branch".into()),
                        );
                        merged_n
                    } else {
                        let new_n = pr_ship_create_pr(&project_root, &branch)?;
                        eprintln!(
                            "  {} created PR-{}",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            new_n
                        );
                        log_ship_activity(
                            &main_worktree,
                            Some(new_n),
                            &ShipStep::ResolvePr {
                                create_if_needed: true,
                            },
                            &StepOutcome::Ok,
                        );
                        new_n
                    }
                }
            }
        }
    };

    // BUG-733: an explicit `aida pr ship <N>` may be run from the PR base
    // branch. Resolve the target PR's head branch before deciding whether to
    // delete it or whether open PRs are stacked on it.
    // trace:BUG-733 | ai:codex
    let branch_for_child_lookup = target_change
        .as_ref()
        .and_then(|c| (!c.branch.is_empty()).then_some(c.branch.as_str()))
        .unwrap_or(&branch);
    let raw_open_child_prs = open_child_prs(&project_root, branch_for_child_lookup);
    let (ship_branch, retarget_base, open_child_prs) =
        pr_ship::ship_branch_context(&branch, target_change.as_ref(), &raw_open_child_prs);

    if ship_branch != branch {
        eprintln!(
            "  step 1: PR-{} head branch is {} (current checkout: {})",
            pr_number, ship_branch, branch
        );
    }

    // ---- Detect: is the branch checked out in a sibling worktree? ----
    // If so, `gh pr merge --delete-branch` will fail the local-cleanup
    // step with "branch X is already used by worktree at Y". We skip
    // `--delete-branch` in that case and let `aida session end` handle
    // the local cleanup (which knows how to remove the worktree first).
    let branch_in_sibling = branch_in_sibling_worktree(&main_worktree, &ship_branch, &project_root);

    // ---- BUG-434: detect branches/PRs stacked ON this branch. ----
    // Deleting the merged branch orphans local children and GitHub
    // auto-CLOSES any PR based on it (the #439 slip). Two substrate
    // sources: (a) `.aida/stacks.json` parent_branch links (STORY-248),
    // (b) `gh pr list --base <branch> --state open`. Either populated ⇒
    // keep the branch unless `--force-delete-branch`. trace:BUG-434
    let stack_graph = stacks::load(&project_root);
    let stacked_children: Vec<String> = stacks::children_of(&stack_graph, &ship_branch)
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let delete_branch = pr_ship::should_delete_branch(
        branch_in_sibling,
        stacked_children.len(),
        open_child_prs.len(),
        force_delete_branch,
    );
    if !delete_branch
        && !branch_in_sibling
        && (!stacked_children.is_empty() || !open_child_prs.is_empty())
    {
        // The merge still proceeds; we just keep the base branch alive so
        // the stacked PRs survive. Tell the operator how to clean up.
        let child_prs: Vec<String> = open_child_prs.iter().map(|n| format!("#{n}")).collect();
        eprintln!(
            "  {} keeping branch `{}` after merge — {} stacked on it",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
            ship_branch,
            if child_prs.is_empty() {
                format!(
                    "{} child branch(es): {}",
                    stacked_children.len(),
                    stacked_children.join(", ")
                )
            } else {
                format!("open child PR(s): {}", child_prs.join(", "))
            }
        );
        if !child_prs.is_empty() {
            eprintln!(
                "    retarget them to {} first ({}), then delete `{}` — or re-run with --force-delete-branch to orphan them.",
                retarget_base,
                open_child_prs
                    .iter()
                    .map(|n| format!("gh pr edit {n} --base {retarget_base}"))
                    .collect::<Vec<_>>()
                    .join("; "),
                ship_branch
            );
        } else {
            eprintln!(
                "    re-target those branches off `{}` before deleting it — or re-run with --force-delete-branch to orphan them.",
                ship_branch
            );
        }
    }

    let steps = vec![
        ShipStep::ResolvePr {
            create_if_needed: opts.pr_number.is_none(),
        },
        ShipStep::WatchCi,
        ShipStep::Merge { delete_branch },
        ShipStep::Pull,
        ShipStep::EndLease,
    ];

    // ---- Dry-run: print the resolved plan and exit. ----
    if dry_run {
        let plan = format_dry_run_plan(
            &opts,
            &steps,
            crate::forge::resolve_forge_kind(&project_root),
        );
        print!("{}", plan);
        eprintln!(
            "  → PR target: {}",
            if pr_number == 0 {
                "<would-create>".to_string()
            } else {
                format!("PR-{}", pr_number)
            }
        );
        return Ok(());
    }

    // ---- BUG-710: substrate-as-bouncer. An implementer running INSIDE an
    // orchestrated HEADLESS drive (`AIDA_HEADLESS=1`) must NOT self-merge its
    // own PR — `aida zen` promises an INDEPENDENT reviewer before the
    // auto-merge, and a phase-1 self-merge bypasses it (the failure the codex
    // TASK-1115/1119 drives exposed). Leave the PR OPEN and STOP so the
    // orchestrator's CI + reviewer + merge phases finish it; the implementer's
    // job was only to open the PR. Sibling of the STORY-529 gate below. One
    // explicit opt-in (`AIDA_PR_SHIP_ALLOW_IN_DRIVE=1`) covers a deliberate
    // headless direct-publish. trace:BUG-710 | ai:claude
    //
    // BUG-716: BUG-710 gated only on AIDA_HEADLESS, so a --supervised
    // (interactive) implementer — which has no AIDA_HEADLESS — slipped past and
    // self-merged (codex TASK-1123, 0 reviews). Also treat a LIVE drain lock as
    // "inside an orchestrated drive": every drive (headless AND supervised)
    // holds it, a plain `aida queue work <spec>` session does not, and a stale
    // post-crash lock (BUG-712) is not `Running`. trace:BUG-716 | ai:claude
    let in_headless_drive = std::env::var("AIDA_HEADLESS")
        .map(|v| v == "1")
        .unwrap_or(false);
    let live_drive = matches!(
        drain_lock::probe_lock(&main_worktree),
        drain_lock::LockStatus::Running(_)
    );
    let in_orchestrated_drive = in_headless_drive || live_drive;
    let allow_in_drive = std::env::var("AIDA_PR_SHIP_ALLOW_IN_DRIVE")
        .map(|v| v == "1")
        .unwrap_or(false);
    if pr_ship::should_block_ship_merge(in_orchestrated_drive, allow_in_drive) {
        eprintln!(
            "{} PR-{} left OPEN — `aida pr ship` will not self-merge inside an \
             orchestrated drive (headless or supervised). The implementer opens \
             the PR and exits; the orchestrator's independent reviewer gates the \
             merge. (Deliberate in-drive direct-publish? set \
             AIDA_PR_SHIP_ALLOW_IN_DRIVE=1.)",
            "⏸".yellow().bold(),
            pr_number,
        );
        log_ship_activity(
            &main_worktree,
            Some(pr_number),
            &pr_ship::ShipStep::Merge { delete_branch },
            &pr_ship::StepOutcome::Skipped(
                "inside an orchestrated drive — the reviewer gates the merge (BUG-710/BUG-716)"
                    .to_string(),
            ),
        );
        return Ok(());
    }

    // ---- STORY-529: draft-for-review gate. A spec tagged `review:draft-only`
    // must NOT be auto-merged by `aida pr ship` — leave the PR a draft and STOP
    // (before CI-watch + merge) so a human reviews + merges. Opt-in via the tag,
    // so untagged specs are unaffected. Enforces the draft-for-review handoff
    // that briefs alone couldn't (handed-off agents self-merged). trace:STORY-529
    let draft_only = draft_only_specs_for_ship(&project_root);
    if !draft_only.is_empty() {
        // Convert the PR back to a draft (no-op/harmless if already one or if
        // the forge doesn't support drafts — the load-bearing effect is the
        // bail before merge below).
        let _ = std::process::Command::new("gh")
            .current_dir(&project_root)
            .args(["pr", "ready", &pr_number.to_string(), "--undo"])
            .status();
        eprintln!(
            "{} PR-{} left as a DRAFT — {} tagged `{}`. A human must review + merge; \
             `aida pr ship` will not auto-merge it.",
            "⏸".yellow().bold(),
            pr_number,
            draft_only.join(", "),
            pr_ship::DRAFT_ONLY_TAG
        );
        log_ship_activity(
            &main_worktree,
            Some(pr_number),
            &pr_ship::ShipStep::Merge { delete_branch },
            &pr_ship::StepOutcome::Skipped(format!(
                "{} tagged review:draft-only — held for human review",
                draft_only.join(", ")
            )),
        );
        return Ok(());
    }

    // ---- BUG-727: supervised-merge gate (substrate-as-bouncer). The keystone
    // "Gate + PR, do NOT merge — advisor reviews first" directive used to live
    // only in spec PROSE, which this ship path never read — it auto-merged
    // keystone work straight past it. The machine-readable marker is the
    // spec's `execution_mode`: only `drain` licenses an unattended merge; any
    // other mode — or none at all (fail safe) — parks the PR OPEN and
    // mergeable, awaiting an explicit human/advisor merge. A human at an
    // interactive TTY IS that explicit merge, so the gate fires only for
    // non-interactive (automation) callers. Skipped when the PR is already
    // merged — there is nothing left to refuse, only the idempotent post-merge
    // sync below. trace:BUG-727 | ai:claude
    let interactive_tty = std::io::IsTerminal::is_terminal(&std::io::stdin())
        && std::io::IsTerminal::is_terminal(&std::io::stdout());
    let supervised = if already_merged {
        Vec::new()
    } else {
        supervised_specs_for_ship(&project_root, interactive_tty)
    };
    if !supervised.is_empty() {
        for (id, label) in &supervised {
            eprintln!(
                "{} {id} is marked {label} — auto-merge refused; merge requires \
                 human/advisor review",
                "⏸".yellow().bold(),
            );
        }
        eprintln!(
            "{} PR-{} left OPEN — mergeable, awaiting human/advisor review. Once \
             reviewed, merge it explicitly: `gh pr merge {} --squash; aida pull`.",
            "⏸".yellow().bold(),
            pr_number,
            pr_number,
        );
        log_ship_activity(
            &main_worktree,
            Some(pr_number),
            &pr_ship::ShipStep::Merge { delete_branch },
            &pr_ship::StepOutcome::Skipped(format!(
                "{} — execution mode holds the merge for human/advisor review",
                supervised
                    .iter()
                    .map(|(id, label)| format!("{id} marked {label}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        );
        return Ok(());
    }

    // BUG-574: if the PR is already merged (re-run on an already-shipped spec,
    // or a sibling beat us to the merge) there is nothing to watch or merge.
    // Skip straight to the idempotent pull + cleanup so the re-run exits 0
    // instead of erroring out of a no-op `gh pr merge`. trace:BUG-574 | ai:claude
    if already_merged {
        eprintln!(
            "  {} PR-{} is already merged — nothing to ship; running post-merge sync + cleanup",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            pr_number
        );
        log_ship_activity(
            &main_worktree,
            Some(pr_number),
            &ShipStep::WatchCi,
            &StepOutcome::Skipped("PR already merged".into()),
        );
        log_ship_activity(
            &main_worktree,
            Some(pr_number),
            &ShipStep::Merge { delete_branch },
            &StepOutcome::Skipped("PR already merged".into()),
        );
    } else if let Some(reason) = pr_ship_ci_wait_skip_reason(&project_root) {
        eprintln!(
            "  step 2: {} — skipping CI wait for PR-{}",
            reason, pr_number
        );
        log_ship_activity(
            &main_worktree,
            Some(pr_number),
            &ShipStep::WatchCi,
            &StepOutcome::Skipped(reason),
        );
    } else {
        eprintln!("  step 2: watching CI for PR-{}", pr_number);
        wait_for_pr_checks_to_register(&project_root, pr_number)?;
        // STORY-516: route the blocking CI watch through the Forge trait (streams
        // live; GitHub `gh pr checks <N> --watch`). trace:STORY-516 | ai:claude
        let watch_change = crate::forge::ChangeRef {
            id: pr_number,
            url: String::new(),
            branch: ship_branch.clone(),
            base: String::new(),
            title: None,
        };
        let ci_result = crate::forge::forge_for(&project_root).watch_ci(&watch_change);
        let ci_failed = matches!(ci_result, Ok(crate::forge::CiState::Failed) | Err(_));
        if ci_failed {
            let detail = match &ci_result {
                Err(e) => format!("{e:#}"),
                _ => format!("CI did not pass for PR-{pr_number}"),
            };
            log_ship_activity(
                &main_worktree,
                Some(pr_number),
                &ShipStep::WatchCi,
                &StepOutcome::Failed(detail),
            );
            let hint = recovery_hint(&ShipStep::WatchCi, Some(pr_number));
            eprintln!(
                "{} CI failed for PR-{}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                pr_number
            );
            eprintln!("  {}", hint);
            anyhow::bail!("CI did not pass for PR-{pr_number}");
        }
        eprintln!(
            "  {} CI green for PR-{}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            pr_number
        );
        log_ship_activity(
            &main_worktree,
            Some(pr_number),
            &ShipStep::WatchCi,
            &StepOutcome::Ok,
        );
    }

    // ---- Step 3: merge. Use retry-wrapper so a transient gh
    // network blip doesn't abort. ----
    // BUG-574: skipped entirely when the PR is already merged — the activity
    // log for the merge step was already recorded as Skipped above, and the
    // idempotent pull + cleanup below still run. trace:BUG-574 | ai:claude
    let mut merged_this_run = false;
    if !already_merged {
        if branch_in_sibling {
            eprintln!(
                "  step 3: branch {} is checked out in a sibling worktree — \
                 skipping `--delete-branch`; `aida session end` will clean it up",
                ship_branch
            );
        }
        eprintln!(
            "  step 3: merging PR-{} (squash{})",
            pr_number,
            if delete_branch { ", delete-branch" } else { "" }
        );
        let explicit_squash_subject =
            derive_pr_ship_squash_subject(&project_root, pr_number, &ship_branch)?;
        if let Some(subject) = &explicit_squash_subject {
            eprintln!(
                "  step 3: preserving spec ID in squash subject: {}",
                subject
            );
        }

        // STORY-516: route the merge through the Forge trait instead of an inline
        // `gh pr merge`. GitHubForge::merge_change reuses the SPEC-410-pinned
        // `merge_args` (byte-identical argv) and the same network_retry wrapper, so
        // this is behaviour-preserving on GitHub; a GitLab / pure-git repo now gets
        // its own provider's merge. The unified contract returns Err (with stderr)
        // on a failed merge, which we map to the existing activity-log + recovery
        // hint + bail. trace:STORY-516 | ai:claude
        let merge_opts = crate::forge::MergeOptions {
            method: crate::forge::MergeMethod::Squash,
            squash_subject: explicit_squash_subject.clone(),
            delete_branch,
        };
        let change_ref = crate::forge::ChangeRef {
            id: pr_number,
            url: String::new(),
            branch: ship_branch.clone(),
            base: retarget_base.clone(),
            title: None,
        };
        let mut merge_sink = crate::network_retry::StderrSink;
        if let Err(e) = crate::forge::forge_for(&project_root).merge_change(
            &change_ref,
            &merge_opts,
            &mut merge_sink,
        ) {
            let stderr_text = format!("{e:#}");
            let mut probe_sink = crate::network_retry::StderrSink;
            if pr_ship::merge_error_landed_despite_failure(pr_is_merged_with_sink(
                &project_root,
                pr_number as u32,
                &mut probe_sink,
            )) {
                eprintln!(
                    "  {} merge command reported an error after PR-{} landed; continuing with post-merge sync",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    pr_number
                );
                eprintln!("    {}", stderr_text);
                log_ship_activity(
                    &main_worktree,
                    Some(pr_number),
                    &ShipStep::Merge { delete_branch },
                    &StepOutcome::Ok,
                );
                merged_this_run = true;
            } else {
                log_ship_activity(
                    &main_worktree,
                    Some(pr_number),
                    &ShipStep::Merge { delete_branch },
                    &StepOutcome::Failed(stderr_text.clone()),
                );
                let hint = recovery_hint(&ShipStep::Merge { delete_branch }, Some(pr_number));
                eprintln!(
                    "{} merge failed: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    stderr_text
                );
                eprintln!("  {}", hint);
                return Err(e.context("`gh pr merge` failed"));
            }
        } else {
            eprintln!(
                "  {} merged PR-{}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                pr_number
            );
            log_ship_activity(
                &main_worktree,
                Some(pr_number),
                &ShipStep::Merge { delete_branch },
                &StepOutcome::Ok,
            );
            merged_this_run = true;
        }
    }

    // STORY-439: ship-side calibration capture. Resolve every spec the PR
    // credits (title → branch → body, the same precedence the squash
    // subject repair already uses) and write a ship slot per spec — the
    // implementer's self-assessed complexity + the punt count
    // (`.aida/punts.jsonl` filtered by spec). One PR crediting N specs
    // populates N records. Best-effort: a `gh` blip here leaves the merge
    // landed and just skips the capture.
    // trace:STORY-439 | ai:claude
    if let Ok(pr_meta) = fetch_pr_ship_metadata_via_gh(&project_root, pr_number) {
        let spec_ids =
            pr_ship::derive_squash_subject_spec_ids(&pr_meta.title, &branch, &pr_meta.body);
        for spec in &spec_ids {
            let punts = complexity_calibration::punt_count_for_spec(&main_worktree, spec);
            if let Err(e) =
                complexity_calibration::upsert_ship(&main_worktree, spec, opts.complexity, punts)
            {
                eprintln!(
                    "  {} could not record ship calibration for {spec}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
            if let Err(e) = effort_calibration::upsert_ship(
                &main_worktree,
                spec,
                opts.effort,
                Some(current_user_id(None)),
            ) {
                eprintln!(
                    "  {} could not record ship effort for {spec}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
            apply_effort_tag(
                &Storage::new(main_worktree.join(".aida-store")),
                spec,
                effort_calibration::EffortTouchpoint::Impl,
                opts.effort,
            );
        }
    }

    // ---- Step 4: aida pull (from the main worktree). ----
    if no_pull {
        eprintln!("  step 4: skipping `aida pull` (--no-pull)");
        log_ship_activity(
            &main_worktree,
            Some(pr_number),
            &ShipStep::Pull,
            &StepOutcome::Skipped("--no-pull".into()),
        );
    } else {
        eprintln!(
            "  step 4: running `aida pull` from main worktree ({})",
            main_worktree.display()
        );
        let pull_status = match prepare_main_worktree_for_pr_ship_pull(&main_worktree) {
            Ok(()) => {
                // Spawn `aida pull` as a subcommand. Use the hardened resolver
                // instead of raw current_exe(): dev rebuilds can make Linux
                // report "<path> (deleted)", which Command::new cannot spawn.
                // trace:SPEC-411 | ai:codex
                let aida_bin = pr_ship_post_merge_aida_exe();
                Some(
                    std::process::Command::new(&aida_bin)
                        .current_dir(&main_worktree)
                        .arg("pull")
                        .status()
                        .context("could not invoke `aida pull`")?,
                )
            }
            Err(e) => {
                log_ship_activity(
                    &main_worktree,
                    Some(pr_number),
                    &ShipStep::Pull,
                    &StepOutcome::Failed(e.to_string()),
                );
                let hint = recovery_hint(&ShipStep::Pull, Some(pr_number));
                eprintln!(
                    "{} could not prepare main worktree for `aida pull` — the merge already landed, so this is a sync issue, not a merge issue.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                );
                eprintln!("  {}", e);
                eprintln!("  {}", hint);
                None
            }
        };
        if let Some(pull_status) = pull_status {
            if !pull_status.success() {
                log_ship_activity(
                    &main_worktree,
                    Some(pr_number),
                    &ShipStep::Pull,
                    &StepOutcome::Failed(format!("exit {}", pull_status)),
                );
                let hint = recovery_hint(&ShipStep::Pull, Some(pr_number));
                eprintln!(
                    "{} `aida pull` failed — the merge already landed, so this \
                     is a sync issue, not a merge issue.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                );
                eprintln!("  {}", hint);
                // Don't bail — the merge landed; pull failure is recoverable.
            } else {
                eprintln!(
                    "  {} pulled main",
                    crate::glyph(crate::glyphs::Glyph::Check).green()
                );
                log_ship_activity(
                    &main_worktree,
                    Some(pr_number),
                    &ShipStep::Pull,
                    &StepOutcome::Ok,
                );
            }
        } else {
            // Preparation failure was already logged and printed above.
        }
    }

    // ---- Step 5: end the lease (worktree cleanup). ----
    if no_cleanup {
        eprintln!("  step 5: skipping `aida session end` (--no-cleanup)");
        log_ship_activity(
            &main_worktree,
            Some(pr_number),
            &ShipStep::EndLease,
            &StepOutcome::Skipped("--no-cleanup".into()),
        );
    } else {
        let lease_for_branch = list_leases(&main_worktree)
            .into_iter()
            .find(|l| l.branch == branch);
        match lease_for_branch {
            Some(lease) => {
                // If our cwd is inside the worktree we're about to
                // remove, `aida session end` will refuse (BUG-61 live-
                // claude check). Detect and skip with a clear next-step.
                let cwd = std::env::current_dir().ok();
                let inside_target = cwd
                    .as_deref()
                    .map(|c| c.starts_with(&lease.worktree_path))
                    .unwrap_or(false);
                if inside_target {
                    eprintln!(
                        "  step 5: lease {} owns this shell's worktree — \
                         exit this shell, then run `aida session end {}`",
                        lease.id, lease.id
                    );
                    log_ship_activity(
                        &main_worktree,
                        Some(pr_number),
                        &ShipStep::EndLease,
                        &StepOutcome::Skipped(format!(
                            "shell inside lease {} — user must run `aida session end {}` after exiting",
                            lease.id, lease.id
                        )),
                    );
                } else {
                    eprintln!(
                        "  step 5: ending lease {} (worktree {})",
                        lease.id,
                        lease.worktree_path.display()
                    );
                    let aida_bin = pr_ship_post_merge_aida_exe();
                    let end_status = std::process::Command::new(&aida_bin)
                        .current_dir(&main_worktree)
                        .args(["session", "end", &lease.id, "--yes", "--skip-ci"])
                        .status()
                        .context("could not invoke `aida session end`")?;
                    if !end_status.success() {
                        log_ship_activity(
                            &main_worktree,
                            Some(pr_number),
                            &ShipStep::EndLease,
                            &StepOutcome::Failed(format!("exit {}", end_status)),
                        );
                        let hint = recovery_hint(&ShipStep::EndLease, Some(pr_number));
                        eprintln!(
                            "{} session end failed",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                        );
                        eprintln!("  {}", hint);
                    } else {
                        eprintln!(
                            "  {} ended lease {}",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            lease.id
                        );
                        log_ship_activity(
                            &main_worktree,
                            Some(pr_number),
                            &ShipStep::EndLease,
                            &StepOutcome::Ok,
                        );
                    }
                }
            }
            None => {
                eprintln!(
                    "  step 5: no AIDA lease for branch {} — nothing to clean up",
                    branch
                );
                log_ship_activity(
                    &main_worktree,
                    Some(pr_number),
                    &ShipStep::EndLease,
                    &StepOutcome::Skipped(format!("no lease for branch {}", branch)),
                );
            }
        }
    }

    // TASK-1145: after an AIDA-managed squash merge lands, opportunistically
    // prune verified merged agent worktrees. This reuses the destructive
    // doctor heal gate, so unattended/autonomous contexts get a visible skip
    // rather than a forced removal. Best-effort: post-merge GC must never turn a
    // landed PR into a failed ship.
    // trace:TASK-1145 | ai:codex
    if merged_this_run {
        eprintln!("  step 6: pruning verified merged agent worktrees");
        if let Err(e) = doctor_cmd::run_merged_agent_worktree_gc(
            /* yes */ true, /* force */ true, /* json */ false,
        ) {
            eprintln!(
                "  {} post-merge worktree gc failed or was partially applied: {e:#}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
    }

    eprintln!(
        "{} aida pr ship — PR-{} shipped",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        pr_number
    );

    // BUG-376: substrate-as-bouncer signal. The implementer's job ends
    // here — CI was gated in step 2, the merge ran in step 3, pull +
    // auto-bump in step 4, lease release in step 5 (any skipped step
    // already reported its alternate path above). The banner exists so
    // a confident LLM that "wants to helpfully watch CI" is told, at
    // the load-bearing moment, that there is nothing left to watch.
    // Paired with the `aida-implement.md` Step 7 skill directive. The
    // banner is the *substrate* half of the pairing — even an agent
    // that has not read or has misremembered the skill template sees
    // this on the way out. trace:BUG-376 | ai:claude
    let mut stderr = std::io::stderr();
    let _ = write_implementer_complete_banner(&mut stderr, pr_number);

    // Keep the activity-event formatter referenced so the warning-as-
    // unused-import doesn't fire if a future refactor narrows usage.
    let _ = format_activity_event;
    Ok(())
}

/// Loud "IMPLEMENTER COMPLETE — EXIT NOW" banner printed at the end of
/// a successful `aida pr ship`. Substrate-as-bouncer signal for BUG-376:
/// an interactive implementer session that has just shipped a PR has
/// nothing left to do and must exit. Lingering to "watch CI" forces a
/// manual Ctrl+D from the operator and prevents the orchestrator from
/// advancing phases.
///
/// Worded so it stays correct when `--no-pull` / `--no-cleanup` / the
/// cwd-inside-worktree fallback caused step 4 or 5 to skip — those
/// alternate paths print their own next-step lines above; this banner
/// just reinforces that *the implementer session itself* is done.
///
/// Takes `&mut impl Write` so the rendering is unit-testable without
/// spawning a subprocess — mirrors the `status_cleanup::render` pattern.
///
/// trace:BUG-376 | ai:claude
pub(crate) fn write_implementer_complete_banner(
    w: &mut impl std::io::Write,
    pr_number: u64,
) -> std::io::Result<()> {
    let bar = "═".repeat(64);
    writeln!(w)?;
    writeln!(w, "{}", bar.bold())?;
    writeln!(
        w,
        "{} {}",
        crate::glyph(crate::glyphs::Glyph::FlowActive).cyan().bold(),
        "IMPLEMENTER COMPLETE — EXIT NOW".bold()
    )?;
    writeln!(w, "{}", bar.bold())?;
    writeln!(w)?;
    writeln!(
        w,
        "  PR-{} is merged. The post-ship phases (CI watch, merge, pull /",
        pr_number
    )?;
    writeln!(
        w,
        "  auto-bump, lease release) either ran in steps 2-5 above or are"
    )?;
    writeln!(
        w,
        "  listed there as their own next-step lines. Nothing left for this"
    )?;
    writeln!(w, "  implementer session to do.")?;
    writeln!(w)?;
    writeln!(
        w,
        "  Do NOT watch CI further — step 2 already gated on green before the"
    )?;
    writeln!(
        w,
        "  merge. Do NOT wait for the merge — step 3 already landed it. Do"
    )?;
    writeln!(
        w,
        "  NOT re-run `aida pull` or `aida status` to verify the auto-bump /"
    )?;
    writeln!(
        w,
        "  cleanup — any skipped step is recoverable from the lines above and"
    )?;
    writeln!(w, "  this session adds nothing by polling for it.")?;
    writeln!(w)?;
    writeln!(
        w,
        "  {} Press Ctrl+D to exit the chat session. Anything that remains is",
        "→".cyan().bold()
    )?;
    writeln!(
        w,
        "    owned by the orchestrator (--auto-complete) or the next-phase"
    )?;
    writeln!(w, "    agent — not this implementer.")?;
    writeln!(w)?;
    Ok(())
}

pub(crate) fn pr_ship_post_merge_aida_exe() -> std::path::PathBuf {
    resolve_aida_exe()
}

/// `gh pr create` with title/body derived from the latest commit on
/// `branch`. Returns the new PR's number, parsed from the URL `gh`
/// prints on success. Branch must already be pushed; we push it first.
/// BUG-574: return the most-recent MERGED PR number for `branch`, if any.
/// Used by `aida pr ship` to recognize an already-shipped branch (no open PR
/// but a merged one) and exit 0 instead of failing in `pr_ship_create_pr`'s
/// "no commits between" path. Best-effort: any `gh` failure (missing binary,
/// auth, network) returns `None`, so a probe blip never converts an otherwise-
/// fine create into a spurious already-merged short-circuit. trace:BUG-574 | ai:claude
pub(crate) fn latest_merged_pr_for_branch(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<u64> {
    // Route the merged-change lookup through the forge (GitHub: `gh pr list
    // --head --state merged`; GitLab: `glab mr list --source-branch --merged`).
    // Only a definitively-Found merged change yields a number; every other state
    // collapses to None, matching the prior raw-gh failure-to-None behaviour.
    // trace:TASK-961 trace:STORY-621 | ai:claude
    match detect_merged_pr_for_branch_via_forge(project_root, branch) {
        PrLookup::Found(info) => Some(info.number),
        _ => None,
    }
}

pub(crate) fn pr_ship_create_pr(project_root: &std::path::Path, branch: &str) -> Result<u64> {
    // STORY-516: PR-number parsing now lives in GitHubForge::open_change, so
    // `parse_pr_number_from_create_output` is no longer needed here.
    use pr_ship::{derive_pr_body_from_commit, derive_pr_title_from_commit};

    // Push first so `gh pr create` doesn't error with "no commits between".
    eprintln!("  pushing {} to origin", branch);
    let push_status = std::process::Command::new("git")
        .current_dir(project_root)
        .args(["push", "-u", "origin", branch])
        .status()
        .context("could not invoke `git push`")?;
    if !push_status.success() {
        anyhow::bail!("`git push -u origin {branch}` failed — investigate before retrying");
    }

    let commit_msg_out = std::process::Command::new("git")
        .current_dir(project_root)
        .args(["log", "-1", "--format=%B"])
        .output()
        .context("could not invoke `git log` to derive PR title/body")?;
    if !commit_msg_out.status.success() {
        anyhow::bail!(
            "`git log -1 --format=%B` failed: {}",
            String::from_utf8_lossy(&commit_msg_out.stderr).trim()
        );
    }
    let commit_msg = String::from_utf8_lossy(&commit_msg_out.stdout).to_string();
    let title = derive_pr_title_from_commit(&commit_msg);
    let body = derive_pr_body_from_commit(&commit_msg);
    if title.is_empty() {
        anyhow::bail!("could not derive a non-empty PR title from the latest commit");
    }

    // STORY-516: route PR creation through the Forge trait. open_change passes
    // an explicit `--head <branch>` (the branch we just pushed) — equivalent to
    // the prior inferred head, and what makes a GitLab/pure-git repo open its
    // own change. Returns ChangeRef{id} (id == 0 on an unparseable URL, which we
    // treat as the prior "no PR number found" bail). trace:STORY-516 | ai:claude
    // BUG-417: resolve the PR base from origin's default branch instead of
    // assuming `main`. A repo whose default is `master` (or anything else) made
    // `gh pr create --base main` fail with a GraphQL base-ref error. trace:BUG-417 | ai:claude
    let base = crate::forge::default_branch_of(project_root);
    eprintln!("  base branch: {}", base);
    let req = crate::forge::OpenChange {
        branch: branch.to_string(),
        base,
        title: title.clone(),
        body,
        draft: false,
    };
    let change = crate::forge::forge_for(project_root)
        .open_change(req)
        .context("could not invoke `gh pr create`")?;
    if change.id == 0 {
        anyhow::bail!(
            "`gh pr create` succeeded but no PR number found in its output: {}",
            change.url
        );
    }
    Ok(change.id)
}

/// Open a change for a just-pushed `queue recover` branch through the forge,
/// deriving title/body from the branch head commit — the forge-routed
/// equivalent of the old `gh pr create --fill` (which has no trait method, but
/// `open_change` + commit-derived title/body is what `pr_ship_create_pr`
/// already does). Prints a forge-aware action hint and returns whether the
/// change opened.
// trace:TASK-961 trace:STORY-621 | ai:claude
pub(crate) fn recover_open_change(probe_repo: &std::path::Path, branch: &str) -> bool {
    use pr_ship::{derive_pr_body_from_commit, derive_pr_title_from_commit};
    let hint = crate::forge::resolve_forge_kind(probe_repo)
        .create_cmd()
        .unwrap_or_else(|| "open change".to_string());
    println!(
        "  {} {}",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
        hint
    );
    let commit_msg = match std::process::Command::new("git")
        .current_dir(probe_repo)
        .args(["log", "-1", "--format=%B"])
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return false,
    };
    let title = derive_pr_title_from_commit(&commit_msg);
    if title.is_empty() {
        return false;
    }
    let req = crate::forge::OpenChange {
        branch: branch.to_string(),
        base: crate::forge::default_branch_of(probe_repo),
        title,
        body: derive_pr_body_from_commit(&commit_msg),
        draft: false,
    };
    crate::forge::forge_for(probe_repo).open_change(req).is_ok()
}

/// TASK-140: the full HEAD commit message of a BRANCH (not the local cwd HEAD).
/// Tries the local ref first, then `origin/<branch>` (shipping a PR whose branch
/// isn't checked out locally). `None` when neither resolves. Caller takes the
/// first line as the squash subject. trace:TASK-140 | ai:claude
pub(crate) fn branch_head_commit_message(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<String> {
    for r in [branch.to_string(), format!("origin/{branch}")] {
        let out = std::process::Command::new("git")
            .current_dir(project_root)
            .args(["log", "-1", "--format=%B", &r])
            .output()
            .ok()?;
        if out.status.success() {
            let msg = String::from_utf8_lossy(&out.stdout).to_string();
            if !msg.trim().is_empty() {
                return Some(msg);
            }
        }
    }
    None
}

/// Return an explicit `gh pr merge --subject` value when GitHub's default
/// branch-head squash subject would drop spec IDs or a better PR title.
// trace:SPEC-410 TASK-142 | ai:codex
pub(crate) fn derive_pr_ship_squash_subject(
    project_root: &std::path::Path,
    pr_number: u64,
    branch: &str,
) -> Result<Option<String>> {
    // TASK-140: compare against the PR BRANCH's head commit, NOT the local cwd
    // HEAD. `aida pr ship` frequently runs from the main worktree, where HEAD is
    // main's tip. TASK-142: prefer the PR title as the explicit squash subject
    // base because the branch head may itself be a merge commit.
    // trace:TASK-140 TASK-142 | ai:codex
    let pr = fetch_pr_ship_metadata_via_gh(project_root, pr_number)?;
    let commit_msg = branch_head_commit_message(project_root, branch).ok_or_else(|| {
        anyhow::anyhow!(
            "could not read the head commit of branch `{branch}` (tried local and origin/) \
             to derive the squash subject"
        )
    })?;
    let current_subject = pr_ship::derive_pr_title_from_commit(&commit_msg);
    let normalized = pr_ship::derive_squash_subject(&pr.title, branch, &pr.body, &commit_msg)
        .unwrap_or_default();
    if normalized.is_empty() {
        return Ok(None);
    }
    if pr_ship::extract_trailing_spec_ids_from_subject(&normalized).is_empty() {
        anyhow::bail!(
            "final squash subject would lack a trailing `(SPEC-ID)` and no spec ID could be derived from PR title, branch name, PR body, or branch head: `{}`",
            normalized
        );
    }
    if normalized == current_subject {
        Ok(None)
    } else {
        Ok(Some(normalized))
    }
}

#[cfg(test)]
#[path = "tests/task_140_squash_subject_tests.rs"]
mod task_140_squash_subject_tests;

pub(crate) struct PrShipMetadata {
    pub(crate) title: String,
    pub(crate) body: String,
}

pub(crate) fn fetch_pr_ship_metadata_via_gh(
    project_root: &std::path::Path,
    n: u64,
) -> Result<PrShipMetadata> {
    let n_str = n.to_string();
    let out = std::process::Command::new("gh")
        .current_dir(project_root)
        .args(["pr", "view", &n_str, "--json", "title,body"])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!("`gh` not on PATH — install from https://cli.github.com/")
            } else {
                anyhow::anyhow!("`gh pr view {}` failed to spawn: {}", n, e)
            }
        })?;
    if !out.status.success() {
        anyhow::bail!(
            "`gh pr view {}` exited {} — {}",
            n,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("`gh pr view {}` returned non-JSON output", n))?;
    Ok(PrShipMetadata {
        title: json
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        body: json
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// BUG-417: return `Some(reason)` when `aida pr ship` should skip the blocking
/// CI wait rather than hang on a repo that will never register checks. Two
/// triggers: (1) an explicit opt-out env (`AIDA_PR_SHIP_NO_CI_WAIT=1` or the
/// `lifecycle:no-ci-wait` token in `AIDA_LIFECYCLE_TAGS`); (2) no GitHub Actions
/// workflow files configured (`.github/workflows/*.{yml,yaml}` absent or empty)
/// — the quizdom "no `.github/workflows` at all" case. `None` ⇒ wait normally.
/// trace:BUG-417 | ai:claude
pub(crate) fn pr_ship_ci_wait_skip_reason(project_root: &std::path::Path) -> Option<String> {
    // Explicit opt-out (env-level honoring of lifecycle:no-ci-wait, which the
    // direct `pr ship` path does not otherwise read from a spec).
    let env_optout = std::env::var("AIDA_PR_SHIP_NO_CI_WAIT")
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !v.is_empty() && v != "0" && v != "false"
        })
        .unwrap_or(false);
    let tag_optout = std::env::var("AIDA_LIFECYCLE_TAGS")
        .map(|v| {
            v.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("lifecycle:no-ci-wait"))
        })
        .unwrap_or(false);
    if env_optout || tag_optout {
        return Some("CI wait disabled (lifecycle:no-ci-wait)".to_string());
    }

    // No workflows configured ⇒ checks will never register.
    if !repo_has_ci_workflows(project_root) {
        return Some("no CI workflows configured (.github/workflows)".to_string());
    }
    None
}

/// BUG-417: true when the project has at least one GitHub Actions workflow file
/// (`.github/workflows/*.{yml,yaml}`). The pure file-name rule lives in
/// `pr_ship::workflow_files_indicate_ci`; this only does the directory read.
/// trace:BUG-417 | ai:claude
pub(crate) fn repo_has_ci_workflows(project_root: &std::path::Path) -> bool {
    let dir = project_root.join(".github").join("workflows");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return false;
    };
    let names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    pr_ship::workflow_files_indicate_ci(names.iter().map(String::as_str))
}

/// Run a `Command` to completion, retrying transient `ETXTBSY`
/// ("Text file busy", `os error 26`) spawn failures.
///
/// BUG-463: on Linux, `exec`ing a file fails with `ETXTBSY` while *any*
/// process still holds that file open for writing. In a parallel test
/// runner (or any multi-threaded program), one thread that writes an
/// executable script can have its still-open writable fd transiently
/// inherited by an unrelated child process a sibling thread `fork`/`exec`s
/// between that fd's `open` and `close`. The borrowing child keeps the
/// file "busy" until it exits, so the writer's own `exec` of the
/// just-written script races and flakes with `ETXTBSY`. The condition is
/// short-lived (the borrowing child exits in milliseconds), so a bounded
/// retry-with-backoff turns the flake into a deterministic success. This
/// also hardens the real `gh` exec (e.g. a freshly-written `gh` wrapper)
/// at negligible cost.
/// trace:BUG-463 | ai:claude
///
/// `libc` is a `cfg(unix)`-only dependency and `ETXTBSY` is a Unix-only errno
/// (Windows can never produce it on spawn), so the errno check is behind a
/// cfg-gated helper — a real check on unix, a compile-time `false` elsewhere —
/// to keep the Windows build green. trace:BUG-468 | ai:claude
#[cfg(unix)]
pub(crate) fn is_etxtbsy(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(libc::ETXTBSY)
}
#[cfg(not(unix))]
pub(crate) fn is_etxtbsy(_e: &std::io::Error) -> bool {
    false
}
pub(crate) fn command_output_retrying_etxtbsy(
    cmd: &mut std::process::Command,
) -> std::io::Result<std::process::Output> {
    const MAX_ATTEMPTS: u32 = 50;
    let mut attempt = 0u32;
    loop {
        match cmd.output() {
            Ok(out) => return Ok(out),
            Err(e) if is_etxtbsy(&e) && attempt < MAX_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(e) => return Err(e),
        }
    }
}

pub(crate) fn wait_for_pr_checks_to_register(
    project_root: &std::path::Path,
    pr_number: u64,
) -> Result<()> {
    wait_for_pr_checks_to_register_with_gh(
        project_root,
        pr_number,
        std::ffi::OsStr::new("gh"),
        std::time::Duration::from_secs(60),
        std::time::Duration::from_secs(10),
    )
}

// trace:BUG-344 | ai:codex
pub(crate) fn wait_for_pr_checks_to_register_with_gh(
    project_root: &std::path::Path,
    pr_number: u64,
    gh_bin: &std::ffi::OsStr,
    timeout: std::time::Duration,
    interval: std::time::Duration,
) -> Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let pr = pr_number.to_string();
        // BUG-463: retry transient `ETXTBSY` so a just-written `gh` wrapper
        // (or a fake one in the parallel test runner) doesn't flake the exec.
        let mut command = std::process::Command::new(gh_bin);
        command
            .current_dir(project_root)
            .args(["pr", "checks", &pr]);
        let out = command_output_retrying_etxtbsy(&mut command)
            .with_context(|| format!("could not invoke `gh pr checks {pr_number}`"))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if pr_ship::gh_pr_checks_output_has_registered_checks(&stdout, &stderr) {
            return Ok(());
        }
        if !out.status.success()
            && !pr_ship::gh_pr_checks_output_is_unregistered(&stdout, &stderr)
            && (!stdout.trim().is_empty() || !stderr.trim().is_empty())
        {
            anyhow::bail!(
                "`gh pr checks {}` failed before CI registration could be inspected: {}",
                pr_number,
                stderr.trim()
            );
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "No CI workflows registered within {}s for PR-{} — verify .github/workflows is configured and GitHub Actions is enabled",
                timeout.as_secs(),
                pr_number
            );
        }
        std::thread::sleep(interval);
    }
}

// trace:BUG-345 | ai:codex
pub(crate) fn prepare_main_worktree_for_pr_ship_pull(
    main_worktree: &std::path::Path,
) -> Result<()> {
    let branch = current_git_branch(main_worktree)?;
    if branch == "main" {
        return Ok(());
    }
    if branch.is_empty() {
        anyhow::bail!(
            "main worktree {} is detached; run `git -C {} checkout main && aida pull` to complete auto-bump",
            main_worktree.display(),
            main_worktree.display()
        );
    }

    let status = std::process::Command::new("git")
        .current_dir(main_worktree)
        .args(["status", "--porcelain"])
        .output()
        .context("could not invoke `git status --porcelain` in main worktree")?;
    if !status.status.success() {
        anyhow::bail!(
            "`git status --porcelain` failed in main worktree: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        );
    }
    if !String::from_utf8_lossy(&status.stdout).trim().is_empty() {
        anyhow::bail!(
            "main worktree on branch `{}` has uncommitted changes; run `git -C {} status`, then `git -C {} checkout main && aida pull` to complete auto-bump",
            branch,
            main_worktree.display(),
            main_worktree.display()
        );
    }

    let checkout = std::process::Command::new("git")
        .current_dir(main_worktree)
        .args(["checkout", "main", "--quiet"])
        .output()
        .context("could not invoke `git checkout main` in main worktree")?;
    if !checkout.status.success() {
        anyhow::bail!(
            "main worktree on stale branch `{}` could not switch to `main`: {}; run `git -C {} checkout main && aida pull` to complete auto-bump",
            branch,
            String::from_utf8_lossy(&checkout.stderr).trim(),
            main_worktree.display()
        );
    }
    Ok(())
}

/// True when `branch` is checked out in a worktree other than the main
/// worktree at `main_worktree`. Used by `aida pr ship` to decide
/// whether `gh pr merge --delete-branch` will fail its local cleanup
/// step (the friction class from TASK-406).
/// BUG-434: open PR numbers whose base branch is `branch` — the PRs GitHub
/// would auto-close if `branch` were deleted. Best-effort: returns empty on
/// any `gh` error (offline, not a GitHub remote, gh absent), so the guard
/// degrades to the stacks.json-only signal rather than blocking the ship.
/// trace:BUG-434 | ai:claude
pub(crate) fn open_child_prs(project_root: &std::path::Path, branch: &str) -> Vec<u64> {
    // Route the stacked-children guard's PR list through the forge (GitHub:
    // `gh pr list --base <branch>`; GitLab: `glab mr list --target-branch`).
    // `open_only` maps to each CLI's default-open listing; a CLI-missing / failed
    // run degrades to an empty Vec, matching the prior raw-gh behaviour (the
    // BUG-434 guard then simply finds no children to protect). trace:TASK-961
    // trace:STORY-621 trace:BUG-434 | ai:claude
    let filter = crate::forge::ChangeFilter {
        base: Some(branch.to_string()),
        open_only: true,
    };
    crate::forge::forge_for(project_root)
        .list_changes(filter)
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.id)
        .collect()
}

/// BUG-733: best-effort metadata lookup for an explicit ship target. `aida pr
/// ship <N>` can be invoked from `main`; the branch deletion guard still needs
/// PR-N's head branch, not the current checkout branch. The existing forge list
/// call returns head/base for open changes on GitHub and GitLab; failures and
/// already-merged changes degrade to the pre-fix current-branch behavior.
// trace:BUG-733 | ai:codex
pub(crate) fn open_change_by_number(
    project_root: &std::path::Path,
    number: u64,
) -> Option<crate::forge::ChangeRef> {
    let filter = crate::forge::ChangeFilter {
        base: None,
        open_only: true,
    };
    crate::forge::forge_for(project_root)
        .list_changes(filter)
        .ok()?
        .into_iter()
        .find(|c| c.id == number)
}

pub(crate) fn branch_in_sibling_worktree(
    main_worktree: &std::path::Path,
    branch: &str,
    project_root: &std::path::Path,
) -> bool {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "list", "--porcelain"])
        .output();
    let Ok(out) = out else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Porcelain output: blank-line-separated records. Each record has
    // a `worktree <path>` line and (when on a named branch) a
    // `branch refs/heads/<name>` line.
    let mut current_path: Option<std::path::PathBuf> = None;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            current_path = Some(std::path::PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch ") {
            let name = rest.strip_prefix("refs/heads/").unwrap_or(rest);
            if name == branch {
                if let Some(p) = &current_path {
                    if p != main_worktree {
                        return true;
                    }
                }
            }
        } else if line.is_empty() {
            current_path = None;
        }
    }
    false
}

/// Append a JSONL activity-log entry to `<main_worktree>/.aida/advisor-activity.jsonl`.
/// Best-effort: silently swallows IO errors — losing a log line must
/// never abort the ship flow. Composes with STORY-405's planned
/// `aida status` activity surface.
pub(crate) fn log_ship_activity(
    main_worktree: &std::path::Path,
    pr_number: Option<u64>,
    step: &pr_ship::ShipStep,
    outcome: &pr_ship::StepOutcome,
) {
    let now = chrono::Utc::now().to_rfc3339();
    let line = pr_ship::format_activity_event(&now, pr_number, step, outcome);
    let aida_dir = main_worktree.join(".aida");
    if std::fs::create_dir_all(&aida_dir).is_err() {
        return;
    }
    let log_path = aida_dir.join("advisor-activity.jsonl");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = writeln!(f, "{}", line);
    }
}

/// `aida pr rebase <N> --check` — print the stale-base / overlap /
/// conflict-prediction report and exit zero. Modifies nothing.
/// trace:TASK-308 | ai:claude
pub(crate) fn pr_rebase_check_report(
    project_root: &std::path::Path,
    info: &pr_rebase::PrInfo,
    origin_base: &str,
    pr_local_branch: &str,
) -> Result<()> {
    use pr_rebase::{parse_merge_tree_conflicts, ConflictPrediction};

    let n = info.n;

    // Behind count: commits on origin/<base> that aren't on the PR.
    let behind_out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-list",
            "--count",
            &format!("{pr_local_branch}..{origin_base}"),
        ])
        .output()?;
    let behind: u32 = String::from_utf8_lossy(&behind_out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    // Files changed by the PR (PR.. base merge-base..HEAD).
    let pr_files = files_in_range(project_root, &format!("{origin_base}...{pr_local_branch}"));
    // Files changed on origin/<base> since the PR forked.
    let base_files = files_in_range(project_root, &format!("{pr_local_branch}...{origin_base}"));
    let pr_set: std::collections::HashSet<&String> = pr_files.iter().collect();
    let overlap: Vec<String> = base_files
        .iter()
        .filter(|f| pr_set.contains(f))
        .cloned()
        .collect();

    // Conflict prediction via `git merge-tree --write-tree`. Modern
    // git (≥2.38) returns the tree SHA on clean merge and exits
    // non-zero with `--name-only` output on conflict. We tolerate
    // older git by falling back to "Unknown" if the command fails.
    let mt = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            origin_base,
            pr_local_branch,
        ])
        .output();
    let prediction = match mt {
        Ok(out) if out.status.success() => ConflictPrediction::Clean,
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let files = parse_merge_tree_conflicts(&stdout);
            if files.is_empty() {
                ConflictPrediction::Unknown
            } else {
                ConflictPrediction::Conflicting { files }
            }
        }
        Err(_) => ConflictPrediction::Unknown,
    };

    println!("{} PR-{}", "Check:".bold(), n);
    println!("  base       {}", origin_base);
    println!("  state      {} behind", behind,);
    println!("  pr files   {}", pr_files.len());
    println!(
        "  overlap    {} file{}",
        overlap.len(),
        if overlap.len() == 1 { "" } else { "s" }
    );
    for f in overlap.iter().take(10) {
        println!("               {}", f.dimmed());
    }
    match &prediction {
        ConflictPrediction::Clean => {
            println!("  predict    {}", "rebase clean (safe)".green());
            if behind > 0 {
                println!(
                    "  {}",
                    format!("Recommendation: rebase needed (run `aida pr rebase {}`)", n).cyan()
                );
            } else {
                println!("  {}", "Recommendation: PR is up to date".dimmed());
            }
        }
        ConflictPrediction::Conflicting { files } => {
            println!(
                "  predict    {} ({} file{})",
                "rebase will conflict".yellow(),
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            );
            for f in files.iter().take(10) {
                println!("               {}", f.dimmed());
            }
            println!(
                "  {}",
                format!(
                    "Recommendation: manual rebase (`aida pr rebase {} --interactive`)",
                    n
                )
                .yellow()
            );
        }
        ConflictPrediction::Unknown => {
            println!(
                "  predict    {} (git merge-tree probe failed; rely on overlap)",
                "unknown".dimmed()
            );
            if overlap.is_empty() {
                println!(
                    "  {}",
                    "Recommendation: likely clean (no file overlap)".dimmed()
                );
            } else {
                println!(
                    "  {}",
                    "Recommendation: overlap detected — rebase may conflict".yellow()
                );
            }
        }
    }
    Ok(())
}

/// STORY-281: pre-flight stale-base check fired before launching the
/// headless reviewer (orchestrator phase 3) or a direct
/// `aida queue work <PR-N> --for reviewer` session. Mirrors the data
/// `pr_rebase_check_report` derives (gh metadata → fetch origin →
/// behind count → PR + base file lists → `git merge-tree` conflict
/// probe), then runs `pr_rebase::classify_stale_base` to map it onto a
/// reviewer-launch verdict.
///
/// Failure modes (gh not installed, PR not found, fetch failure)
/// surface as `Err(anyhow!)` — caller decides whether to refuse the
/// reviewer or proceed. The convention here is "fail open with a
/// warning" so a transient network blip never blocks an autonomous
/// drain, but a stale-base + overlap that we *did* successfully
/// detect always blocks. trace:STORY-281 | ai:claude
pub(crate) fn preflight_stale_base_check(
    project_root: &std::path::Path,
    n: u64,
) -> Result<pr_rebase::StaleBaseOutcome> {
    // STORY-621 Slice 2: the metadata read is forge-routed (GitLab-safe); the
    // git probing below was already forge-agnostic. trace:TASK-963 | ai:claude
    let info = fetch_change_info_via_forge(project_root, n)?;
    preflight_stale_base_check_with_info(project_root, n, info)
}

/// Test seam: same check with the metadata fetched via an injectable `gh`
/// binary (the task-471 tests fake gh with a script), bypassing the forge
/// routing the production wrapper uses.
#[allow(dead_code)] // exercised by the task-471 tests (cfg(test)-only callers)
pub(crate) fn preflight_stale_base_check_with_gh(
    project_root: &std::path::Path,
    n: u64,
    gh_bin: &std::ffi::OsStr,
) -> Result<pr_rebase::StaleBaseOutcome> {
    let info = fetch_pr_info_via_gh_bin(project_root, n, gh_bin)
        .and_then(|j| pr_rebase::parse_pr_info(&j, n).map_err(|e| anyhow::anyhow!(e)))
        .context("could not resolve PR metadata — is `gh` installed and authenticated?")?;
    preflight_stale_base_check_with_info(project_root, n, info)
}

pub(crate) fn preflight_stale_base_check_with_info(
    project_root: &std::path::Path,
    n: u64,
    info: pr_rebase::PrInfo,
) -> Result<pr_rebase::StaleBaseOutcome> {
    use pr_rebase::{classify_stale_base, parse_merge_tree_conflicts, ConflictPrediction};

    let origin_base = format!("origin/{}", info.base_ref);

    // Refresh origin so origin/<base> is current, and fetch the PR's
    // head ref into a local branch we can probe against.
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["fetch", "origin", "--prune"])
        .status();
    let pr_local_branch = format!("pr-{n}");
    let pr_refspec = format!("+refs/pull/{n}/head:refs/heads/{pr_local_branch}");
    let pr_fetch = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["fetch", "origin", pr_refspec.as_str()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !matches!(pr_fetch, Ok(s) if s.success()) {
        anyhow::bail!(
            "could not fetch PR-{n}'s head ref (`refs/pull/{n}/head`) for pre-flight \
             stale-base check"
        );
    }

    let behind: u32 = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-list",
            "--count",
            &format!("{pr_local_branch}..{origin_base}"),
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse()
                .unwrap_or(0)
        })
        .unwrap_or(0);

    let (pr_files, base_files) =
        preflight_stale_base_file_sets(project_root, &origin_base, &pr_local_branch);

    // git merge-tree probe — same as pr_rebase_check_report.
    let mt = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            &origin_base,
            &pr_local_branch,
        ])
        .output();
    let prediction = match mt {
        Ok(out) if out.status.success() => ConflictPrediction::Clean,
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let files = parse_merge_tree_conflicts(&stdout);
            if files.is_empty() {
                ConflictPrediction::Unknown
            } else {
                ConflictPrediction::Conflicting { files }
            }
        }
        Err(_) => ConflictPrediction::Unknown,
    };

    Ok(classify_stale_base(
        behind,
        &pr_files,
        &base_files,
        prediction,
    ))
}

/// Files touched by `git log --name-only --pretty=format:` over a range.
/// Local helper for `pr_rebase_check_report` so we don't depend on
/// `aida_core::rebase`'s internal helpers (which are crate-private).
/// trace:TASK-308 | ai:claude
pub(crate) fn files_in_range(repo: &std::path::Path, range: &str) -> Vec<String> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "--name-only", "--pretty=format:", range])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut out: Vec<String> = Vec::new();
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                let l = line.trim();
                if !l.is_empty() && seen.insert(l.to_string()) {
                    out.push(l.to_string());
                }
            }
            out
        })
        .unwrap_or_default()
}

pub(crate) fn preflight_stale_base_file_sets(
    repo: &std::path::Path,
    origin_base: &str,
    pr_branch: &str,
) -> (Vec<String>, Vec<String>) {
    (
        files_in_range(repo, &format!("{origin_base}..{pr_branch}")),
        files_in_range(repo, &format!("{pr_branch}..{origin_base}")),
    )
}

/// TASK-480: reviewer pre-flight intermediate-only check. Fetches the
/// PR's head, computes the files it changes against its base, asks the
/// repo's own `.gitignore` whether each is ignored, and runs the pure
/// [`pr_rebase::classify_intermediate_only`] classifier.
///
/// Same fail-open convention as [`preflight_stale_base_check`]: gh /
/// git infra errors surface as `Err` and the caller proceeds with a
/// warning — we never block an autonomous drain on a transient blip.
/// A *successfully detected* intermediate-only diff always refuses.
/// trace:TASK-480 | ai:claude
pub(crate) fn preflight_intermediate_only_check(
    project_root: &std::path::Path,
    n: u64,
) -> Result<pr_rebase::IntermediateOnlyOutcome> {
    // STORY-621 Slice 2: metadata read forge-routed, mirroring
    // preflight_stale_base_check. trace:TASK-963 | ai:claude
    let info = fetch_change_info_via_forge(project_root, n)?;
    preflight_intermediate_only_check_with_info(project_root, n, info)
}

/// Test seam mirroring [`preflight_stale_base_check_with_gh`].
#[allow(dead_code)] // exercised by tests; kept symmetric with the stale-base seam
pub(crate) fn preflight_intermediate_only_check_with_gh(
    project_root: &std::path::Path,
    n: u64,
    gh_bin: &std::ffi::OsStr,
) -> Result<pr_rebase::IntermediateOnlyOutcome> {
    let info = fetch_pr_info_via_gh_bin(project_root, n, gh_bin)
        .and_then(|j| pr_rebase::parse_pr_info(&j, n).map_err(|e| anyhow::anyhow!(e)))
        .context("could not resolve PR metadata — is `gh` installed and authenticated?")?;
    preflight_intermediate_only_check_with_info(project_root, n, info)
}

pub(crate) fn preflight_intermediate_only_check_with_info(
    project_root: &std::path::Path,
    n: u64,
    info: pr_rebase::PrInfo,
) -> Result<pr_rebase::IntermediateOnlyOutcome> {
    let origin_base = format!("origin/{}", info.base_ref);

    // Refresh origin + fetch the PR head, same as the stale-base probe.
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["fetch", "origin", "--prune"])
        .status();
    let pr_local_branch = format!("pr-{n}");
    let pr_refspec = format!("+refs/pull/{n}/head:refs/heads/{pr_local_branch}");
    let pr_fetch = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["fetch", "origin", pr_refspec.as_str()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !matches!(pr_fetch, Ok(s) if s.success()) {
        anyhow::bail!(
            "could not fetch PR-{n}'s head ref (`refs/pull/{n}/head`) for pre-flight \
             intermediate-only check"
        );
    }

    let changed = files_in_range(project_root, &format!("{origin_base}..{pr_local_branch}"));
    Ok(classify_intermediate_only_with_gitignore(
        project_root,
        &changed,
    ))
}

/// Run the pure classifier with a `git check-ignore`-backed gitignore
/// predicate. Pulled out so the wiring (build the predicate, call the
/// classifier) is reused by the preflight and unit-testable in
/// isolation. trace:TASK-480 | ai:claude
pub(crate) fn classify_intermediate_only_with_gitignore(
    project_root: &std::path::Path,
    changed: &[String],
) -> pr_rebase::IntermediateOnlyOutcome {
    pr_rebase::classify_intermediate_only(changed, |path| git_path_is_ignored(project_root, path))
}

/// `git check-ignore -q <path>` — exit 0 ⇒ ignored. We pass `--no-index`
/// so a path that is *tracked* but matches an ignore rule still reports
/// its ignore status (we OR this with the generated-path heuristic; a
/// tracked-but-ignored build output is exactly the intermediate case we
/// want to catch). Any spawn/other error ⇒ treat as not-ignored (fail
/// open). trace:TASK-480 | ai:claude
pub(crate) fn git_path_is_ignored(project_root: &std::path::Path, path: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["check-ignore", "-q", "--no-index", path])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "tests/task_471_stale_base_preflight_tests.rs"]
mod task_471_stale_base_preflight_tests;

#[cfg(test)]
#[path = "tests/story_429_auto_rebase_tests.rs"]
mod story_429_auto_rebase_tests;

#[cfg(all(test, unix))]
mod pr_ship_environment_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    /// BUG-289: a worktree-conflict fetch failure gets the actionable hint
    /// (end the lease / remove the worktree), NOT the misleading "is the PR
    /// number correct?" line; other failures still get the generic hint.
    #[test]
    fn pr_fetch_failure_message_branches_on_worktree_conflict() {
        let worktree_err = "fatal: refusing to fetch into branch 'refs/heads/pr-161' \
                            checked out at '/home/joe/ai/aida-pr-161'";
        let m = pr_fetch_failure_message(worktree_err, 161, "pr-161");
        assert!(
            m.contains("worktree already holds the `pr-161` branch"),
            "{m}"
        );
        assert!(
            m.contains("aida session end") || m.contains("git worktree remove"),
            "{m}"
        );
        assert!(
            !m.contains("is the PR number correct"),
            "worktree-conflict hint must not show the misleading PR-number line: {m}"
        );

        let other_err = "fatal: couldn't find remote ref refs/pull/999/head";
        let g = pr_fetch_failure_message(other_err, 999, "pr-999");
        assert!(g.contains("is the PR number correct"), "{g}");
        assert!(!g.contains("worktree already holds"), "{g}");
    }

    fn git(repo: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {e}", args));
        assert!(
            out.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn init_repo(path: &std::path::Path) {
        git(path, &["init", "-b", "main", "--quiet"]);
        git(path, &["config", "user.email", "aida@example.test"]);
        git(path, &["config", "user.name", "AIDA Test"]);
        git(path, &["commit", "--allow-empty", "-m", "base", "--quiet"]);
    }

    // BUG-463: each test owns its own `tempfile::tempdir()`, so the fake `gh`
    // scripts are already path-isolated. The residual CI flake came from
    // `ETXTBSY` when exec'ing a just-written script while a sibling test's
    // child transiently held the writable fd; the exec path now retries on
    // `ETXTBSY` (see `command_output_retrying_etxtbsy`), so writing the
    // script here stays a plain write + chmod.
    // trace:BUG-463 | ai:claude
    fn make_executable(path: &std::path::Path, body: &str) {
        std::fs::write(path, body).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn waits_for_ci_checks_to_register_before_watch() {
        let tmp = tempfile::tempdir().unwrap();
        let counter = tmp.path().join("count");
        let gh = tmp.path().join("gh");
        make_executable(
            &gh,
            &format!(
                "#!/bin/sh\n\
                 count_file='{}'\n\
                 count=$(cat \"$count_file\" 2>/dev/null || echo 0)\n\
                 count=$((count + 1))\n\
                 printf '%s' \"$count\" > \"$count_file\"\n\
                 if [ \"$count\" -lt 3 ]; then\n\
                   echo \"no checks reported on the branch\" >&2\n\
                   exit 1\n\
                 fi\n\
                 echo \"build\tpending\t0\thttps://github.example/run/1\"\n\
                 exit 0\n",
                counter.display()
            ),
        );

        wait_for_pr_checks_to_register_with_gh(
            tmp.path(),
            344,
            gh.as_os_str(),
            Duration::from_secs(2),
            Duration::from_millis(1),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(counter).unwrap(), "3");
    }

    #[test]
    fn ci_startup_timeout_has_configuration_message() {
        let tmp = tempfile::tempdir().unwrap();
        let gh = tmp.path().join("gh");
        make_executable(
            &gh,
            "#!/bin/sh\n\
             echo \"no checks reported on the branch\" >&2\n\
             exit 1\n",
        );

        let err = wait_for_pr_checks_to_register_with_gh(
            tmp.path(),
            344,
            gh.as_os_str(),
            Duration::from_millis(5),
            Duration::from_millis(1),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("No CI workflows registered"), "{err}");
        assert!(err.contains(".github/workflows"), "{err}");
    }

    #[test]
    fn post_merge_pull_preparation_switches_clean_main_worktree_to_main() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git(tmp.path(), &["checkout", "-b", "stale-feature", "--quiet"]);

        prepare_main_worktree_for_pr_ship_pull(tmp.path()).unwrap();
        assert_eq!(current_git_branch(tmp.path()).unwrap(), "main");
    }

    #[test]
    fn post_merge_pull_preparation_refuses_dirty_feature_branch() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git(tmp.path(), &["checkout", "-b", "stale-feature", "--quiet"]);
        std::fs::write(tmp.path().join("dirty.txt"), "uncommitted\n").unwrap();

        let err = prepare_main_worktree_for_pr_ship_pull(tmp.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("uncommitted changes"), "{err}");
        assert_eq!(current_git_branch(tmp.path()).unwrap(), "stale-feature");
    }

    #[test]
    fn branch_held_by_sibling_worktree_disables_merge_delete_branch() {
        // BUG-732: this is the normal `aida session start` shape. Passing
        // `--delete-branch` to `gh pr merge` makes the remote squash merge land
        // and then fail local branch cleanup because the branch is checked out
        // by the sibling worktree.
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let sibling = tmp.path().join("feature-worktree");
        git(
            tmp.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "feature-branch",
                sibling.to_str().unwrap(),
            ],
        );

        let branch_in_sibling =
            branch_in_sibling_worktree(tmp.path(), "feature-branch", tmp.path());
        assert!(branch_in_sibling);

        let delete_branch = pr_ship::should_delete_branch(branch_in_sibling, 0, 0, false);
        assert!(!delete_branch);
        let merge_args = pr_ship::merge_args(732, delete_branch, None);
        assert!(!merge_args.iter().any(|arg| arg == "--delete-branch"));
    }
}

/// Implementation of `aida pr auto-queue-review` — files (or skips, if
/// already filed) the reviewer story for the PR open on `branch` (default
/// to `git branch --show-current`). Designed to be invoked by /aida-pr
/// right after `gh pr create` returns. trace:STORY-90 | ai:claude
pub(crate) fn pr_auto_queue_review(branch_override: Option<&str>) -> Result<()> {
    let project_root = find_project_root()?;

    let branch = match branch_override {
        Some(b) => b.to_string(),
        None => current_git_branch(&project_root)?,
    };
    if branch.is_empty() {
        anyhow::bail!("could not resolve current branch — pass `--branch <name>` explicitly");
    }

    // Try to associate this run with the active session lease so the
    // review-story description records who opened the PR. Falls back to
    // a synthetic "(no-session)" id if no lease covers the cwd.
    let lease = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(&project_root, &cwd));
    let lease_scope = lease.as_ref().map(|l| l.scope.clone());
    let session_id = lease
        .map(|l| l.id)
        .unwrap_or_else(|| "(no-sess)".to_string());

    let outcome = try_auto_queue_pr_review(
        &project_root,
        &branch,
        &session_id,
        AutoQueueOrigin::PrSkill,
    );
    render_auto_queue_outcome(&outcome);

    // TASK-630: opening the PR is the event that ends a deliberate PR-hold
    // (BUG-250). `aida pr auto-queue-review` runs right after `gh pr create`
    // (the /aida-pr step), so a Filed/AlreadyExists outcome means the held PR is
    // now open — clear the persisted `.aida/pr-holds/<spec>.json` marker so a
    // later `aida queue work <spec> --resume` no longer treats the spec as held.
    // Best-effort: a missing marker (the common case — most PRs were never
    // held) or an unlink error never perturbs the queue-review flow.
    // trace:TASK-630 | ai:claude
    if matches!(
        outcome.status,
        AutoQueueStatus::Filed | AutoQueueStatus::AlreadyExists
    ) {
        if let Some(spec) = &lease_scope {
            let main_root = find_main_worktree_root().unwrap_or_else(|_| project_root.clone());
            let marker = punt::hold_signal_path(&main_root, spec);
            if punt::read_hold_signal(&marker).is_some() {
                let _ = std::fs::remove_file(&marker);
            }
        }
    }

    // BUG-86: on every non-success outcome (needs-attention OR by-design
    // skip), print the exact command to re-run manually. The skill side
    // (/aida-pr step 10) consumes this so the agent can quote it back at
    // the user instead of having them spelunk for the right invocation.
    // Filed and AlreadyExists are the only "no action needed" cases.
    // trace:BUG-86 | ai:claude
    match outcome.status {
        AutoQueueStatus::Filed | AutoQueueStatus::AlreadyExists => {}
        AutoQueueStatus::SkippedByDesign => {
            eprintln!(
                "  {} `aida pr auto-queue-review --branch {}`",
                "Re-run manually:".dimmed(),
                branch
            );
        }
        AutoQueueStatus::SkippedNeedsAttention => {
            eprintln!(
                "  {} `aida pr auto-queue-review --branch {}`",
                "Re-run manually:".yellow().bold(),
                branch
            );
        }
    }

    // Exit non-zero for the "needs attention" bucket so /aida-pr can
    // detect the failure mode and tell the user to install gh / re-auth
    // / etc. Filed, already-filed, and by-design skips all return 0 —
    // they're all expected outcomes the skill should treat as success.
    match outcome.status {
        AutoQueueStatus::SkippedNeedsAttention => {
            anyhow::bail!("{}", outcome.summary)
        }
        _ => Ok(()),
    }
}

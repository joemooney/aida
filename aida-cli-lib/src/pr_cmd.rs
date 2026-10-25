//! `aida pr` command cluster — the pr dispatcher (`handle_pr_command`) and
//! its hold/rebase/ship/auto-queue-review handler layer, extracted from
//! `lib.rs` (SPIKE-78 / STORY-771; pure movement, no behavior change).
//! Shared PR-detection/git helpers stay in `lib.rs`, reached via `crate::`.
// trace:STORY-771 | ai:claude

use crate::process_retry::RetryEtxtbsy;
use crate::*;
use aida_core::git_ops::RemoteCredentialsExt as _;
use serde::{Deserialize, Serialize};

// trace:STORY-1728 | ai:claude
#[path = "pr_cmd_args.rs"]
pub(crate) mod pr_cmd_args;
pub(crate) use pr_cmd_args::*;

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
            override_stale_check,
            override_stale_approval,
            wait,
            no_complete,
            rebase,
            merge_method,
            final_slice,
        } => {
            let resolved_method =
                pr_ship::validate_ship_merge_method(*rebase, *merge_method, *final_slice)
                    .map_err(anyhow::Error::msg)?;
            pr_ship_with_method(
                *n,
                *no_pull,
                *no_cleanup,
                *dry_run,
                *force_delete_branch,
                *complexity,
                *effort,
                *no_trailer_check,
                *override_stale_check,
                *override_stale_approval,
                *wait,
                *no_complete,
                resolved_method,
                *final_slice,
            )
        }
        PrCommand::Train {
            prs,
            all_ready,
            dry_run,
            local_ci,
            max,
            no_pull,
        } => crate::pr_train::pr_train_handler(&crate::pr_train::TrainOptions {
            prs,
            all_ready: *all_ready,
            dry_run: *dry_run,
            local_ci: local_ci.as_deref(),
            max: *max,
            no_pull: *no_pull,
        }),
        // trace:STORY-1887 | ai:claude
        PrCommand::ShipBatch {
            prs,
            review_timeout,
            dry_run,
        } => crate::pr_ship_batch::run(prs, *review_timeout, *dry_run),
        PrCommand::Hold { reason } => pr_hold_handler(reason.as_deref()),
        PrCommand::Gc { dry_run } => pr_gc_handler(*dry_run),
        PrCommand::List { json, mapped_only } => pr_list_handler(*json, *mapped_only),
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

// ---------------------------------------------------------------------------
// TASK-1312: `aida pr gc` — sweep local review-snapshot branches whose PR/MR
// has reached a terminal state.
//
// Every `pr-N` (GitHub) / `mr-N` (GitLab) local branch this project creates
// comes from fetching a forge change's head ref for headless review
// (`aida session start --owns PR-N`, `aida pr rebase`; see BUG-229). Nothing
// deletes it afterward, so a long-lived project's local branch namespace
// accumulates one per review, without bound. `aida pr gc` is the opt-in
// sweep: it never runs on its own, and it only ever touches branches whose
// name is exactly that shape.
// ---------------------------------------------------------------------------

/// Disposition for one candidate review-snapshot branch. Kept separate from
/// the git/forge I/O so the decision itself is a pure, table-testable
/// function.
// trace:TASK-1312 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrGcAction {
    /// Safe to delete: the change is merged/closed, the branch isn't
    /// checked out anywhere, and its tip still matches the change's last
    /// known head SHA.
    Delete,
    /// The change is still open — a review could still need this snapshot.
    SkipOpen,
    /// The branch is checked out in some worktree right now.
    SkipCheckedOut,
    /// The branch's tip no longer matches the change's head SHA (local
    /// commits, or the change moved since the fetch) — not provably a pure
    /// snapshot anymore, so it's left alone rather than guessed at.
    SkipDiverged {
        local_tip: String,
        remote_head: String,
    },
    /// Couldn't resolve the change's state (no forge CLI, auth failure,
    /// network error, deleted PR, …). Fails closed — nothing is deleted
    /// when the terminal-state check itself failed.
    SkipUnknownState(String),
}

/// Pure classifier: given what's already been observed about one candidate
/// branch, decide whether it's safe to delete. No I/O.
// trace:TASK-1312 | ai:claude
pub(crate) fn classify_pr_gc_branch(
    checked_out: bool,
    state: Result<crate::forge::ChangeState, String>,
    local_tip: &str,
    remote_head_sha: &str,
) -> PrGcAction {
    if checked_out {
        return PrGcAction::SkipCheckedOut;
    }
    match state {
        Err(reason) => PrGcAction::SkipUnknownState(reason),
        Ok(crate::forge::ChangeState::Open) => PrGcAction::SkipOpen,
        Ok(crate::forge::ChangeState::Merged) | Ok(crate::forge::ChangeState::Closed) => {
            if !remote_head_sha.is_empty() && local_tip == remote_head_sha {
                PrGcAction::Delete
            } else {
                PrGcAction::SkipDiverged {
                    local_tip: local_tip.to_string(),
                    remote_head: remote_head_sha.to_string(),
                }
            }
        }
    }
}

/// Parse a local branch name as a review-snapshot ref: exactly `pr-<digits>`
/// (GitHub) or `mr-<digits>` (GitLab) — the shape `ReviewForge::local_branch_for`
/// creates. Anything else (an authored spec branch that happens to contain
/// "pr", `pr-161-fixup`, …) is out of scope by construction.
// trace:TASK-1312 | ai:claude
pub(crate) fn parse_review_snapshot_branch(branch: &str) -> Option<(crate::forge::ForgeKind, u64)> {
    let (prefix_len, kind) = if branch.starts_with("pr-") {
        (3, crate::forge::ForgeKind::GitHub)
    } else if branch.starts_with("mr-") {
        (3, crate::forge::ForgeKind::GitLab)
    } else {
        return None;
    };
    let rest = &branch[prefix_len..];
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse::<u64>().ok().map(|n| (kind, n))
}

/// List local branches matching the review-snapshot shape.
// trace:TASK-1312 | ai:claude
fn pr_gc_candidate_branches(project_root: &std::path::Path) -> Result<Vec<String>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .output()
        .context("running git for-each-ref")?;
    anyhow::ensure!(
        out.status.success(),
        "git for-each-ref failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|b| parse_review_snapshot_branch(b).is_some())
        .collect())
}

/// Is `branch` checked out in any worktree of this repo right now?
// trace:TASK-1312 | ai:claude
fn pr_gc_branch_checked_out(project_root: &std::path::Path, branch: &str) -> Result<bool> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .context("running git worktree list")?;
    anyhow::ensure!(
        out.status.success(),
        "git worktree list failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let needle = format!("branch refs/heads/{branch}");
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|l| l.trim() == needle))
}

/// Resolve a local branch's tip SHA.
// trace:TASK-1312 | ai:claude
fn pr_gc_branch_tip(project_root: &std::path::Path, branch: &str) -> Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", &format!("refs/heads/{branch}")])
        .output()
        .context("running git rev-parse")?;
    anyhow::ensure!(
        out.status.success(),
        "git rev-parse {branch} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn pr_gc_delete_branch(project_root: &std::path::Path, branch: &str) -> Result<()> {
    // trace:BUG-2211 | ai:claude
    let out = aida_core::git_ops::output_healing_config_lock(
        project_root,
        || {
            let mut cmd = std::process::Command::new("git");
            cmd.arg("-C").arg(project_root);
            cmd
        },
        &["branch", "-D", branch],
    )
    .context("running git branch -D")?;
    anyhow::ensure!(
        out.status.success(),
        "git branch -D {branch} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

/// Resolve a candidate's terminal state + head SHA via the matching forge
/// CLI in one call. `Err` (stringified — `PrGcAction` doesn't need the full
/// error chain) on any failure to resolve, so the caller fails closed.
// trace:TASK-1312 | ai:claude
fn pr_gc_resolve_metadata(
    project_root: &std::path::Path,
    kind: crate::forge::ForgeKind,
    n: u64,
) -> Result<crate::forge::ChangeMetadata, String> {
    crate::forge::forge_for_kind(project_root, kind)
        .change_metadata(n, &mut network_retry::NoopSink)
        .map_err(|e| e.to_string())
}

/// `aida pr gc` handler: sweep local `pr-N` / `mr-N` branches whose change
/// has reached a terminal state.
// trace:TASK-1312 | ai:claude
pub(crate) fn pr_gc_handler(dry_run: bool) -> Result<()> {
    let project_root = find_main_worktree_root()?;
    let candidates = pr_gc_candidate_branches(&project_root)?;
    if candidates.is_empty() {
        println!("no local `pr-N` / `mr-N` review-snapshot branches found — nothing to sweep.");
        return Ok(());
    }

    let mut deleted = 0usize;
    let mut skipped = 0usize;
    for branch in &candidates {
        let Some((kind, n)) = parse_review_snapshot_branch(branch) else {
            continue;
        };
        let checked_out = pr_gc_branch_checked_out(&project_root, branch)?;
        let local_tip = pr_gc_branch_tip(&project_root, branch)?;
        let (state, remote_head) = if checked_out {
            // Skip the network round-trip — checked-out always skips.
            (Err(String::new()), String::new())
        } else {
            match pr_gc_resolve_metadata(&project_root, kind, n) {
                Ok(m) => (Ok(m.state), m.head_sha),
                Err(e) => (Err(e), String::new()),
            }
        };
        let action = classify_pr_gc_branch(checked_out, state, &local_tip, &remote_head);
        match action {
            PrGcAction::Delete => {
                if dry_run {
                    println!(
                        "  would delete `{branch}` — {} merged/closed, matches fetched head",
                        kind.change_noun()
                    );
                } else {
                    pr_gc_delete_branch(&project_root, branch)?;
                    println!("  {} deleted `{branch}`", "✓".green());
                }
                deleted += 1;
            }
            PrGcAction::SkipOpen => {
                println!("  skip `{branch}` — {} still open", kind.change_noun());
                skipped += 1;
            }
            PrGcAction::SkipCheckedOut => {
                println!("  skip `{branch}` — checked out in a worktree");
                skipped += 1;
            }
            PrGcAction::SkipDiverged {
                local_tip,
                remote_head,
            } => {
                println!(
                    "  skip `{branch}` — has commits not on its remote head (local {}, remote {})",
                    &local_tip[..local_tip.len().min(12)],
                    if remote_head.is_empty() {
                        "unknown".to_string()
                    } else {
                        remote_head[..remote_head.len().min(12)].to_string()
                    }
                );
                skipped += 1;
            }
            PrGcAction::SkipUnknownState(reason) => {
                println!(
                    "  skip `{branch}` — could not resolve {} state ({reason})",
                    kind.change_noun()
                );
                skipped += 1;
            }
        }
    }

    println!();
    if dry_run {
        println!(
            "{deleted} branch(es) would be deleted, {skipped} skipped (dry run — nothing changed)."
        );
    } else {
        println!("{deleted} branch(es) deleted, {skipped} skipped.");
    }
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

/// Preserve the actionable refusal text after the disposable rebase worktree
/// is removed. Best-effort: cleanup must still happen if logging fails.
// trace:BUG-1218 | ai:codex
fn preserve_pr_rebase_diagnostic(project_root: &std::path::Path, n: u64, detail: &str) {
    use std::io::Write;
    let dir = project_root.join(".aida").join("logs");
    if std::fs::create_dir_all(&dir).is_ok() {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("pr-rebase-{n}.log")))
        {
            let _ = writeln!(file, "{}\n", detail.trim());
        }
    }
}

/// BUG-289: build the `aida pr rebase` fetch-failure message. When git's
/// stderr shows the branch is checked out in a worktree (the pr-N reviewer
/// worktree), surface a clear, actionable hint — end the lease or remove the
/// worktree — instead of the misleading "is the PR number correct?" line.
/// trace:BUG-289 | ai:claude
pub(crate) fn pr_fetch_failure_message(stderr: &str, n: u64, pr_local_branch: &str) -> String {
    // external-prose-classifier: pr_cmd::pr_fetch_failure_message
    if aida_core::external_tool_output::contains_any_case_insensitive(
        stderr,
        aida_core::external_tool_output::GIT_BRANCH_CHECKED_OUT,
    ) {
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
            "could not fetch PR-{n}'s head ref — \
             is the PR number correct and the remote reachable?"
        )
    }
}

/// Does `git status --porcelain=v2 --branch` output say a rebase is still in
/// progress (detached head or an unmerged entry)? A status that could not run,
/// failed or was refused (a seat clone, an inexpressible `.gitmodules`) counts
/// as still rebasing, so the caller never proceeds to force-push on it.
// trace:BUG-1985 | ai:claude
pub(crate) fn status_output_says_rebasing(out: std::io::Result<std::process::Output>) -> bool {
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains("# branch.head (no branch)") || s.lines().any(|l| l.starts_with("u "))
        }
        _ => true,
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

    // trace:BUG-1622 | ai:claude
    if let Some(sha) = onto_parent {
        crate::git_arg_guard::reject_option_like("--onto-parent", sha)?;
    }

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
        // trace:BUG-2181 | ai:claude
        .remote_credentials(&project_root, "origin")
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
    // BUG-1228: forge-aware head ref (refs/merge-requests/N/head on GitLab).
    let pr_head_ref = crate::pr_head_remote_ref(&project_root, n);
    let pr_refspec = format!("+{pr_head_ref}:refs/heads/{pr_local_branch}");
    // BUG-289: capture stderr so the failure hint can branch on the actual
    // git error — git refuses to fetch into a local branch that's checked out
    // in a worktree (typically the pr-N reviewer worktree), which is a
    // different problem than a bad PR number / unreachable remote.
    let pr_fetch = std::process::Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args(["fetch", "origin", pr_refspec.as_str()])
        // trace:BUG-2181 | ai:claude
        .remote_credentials(&project_root, "origin")
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
    aida_core::git_ops::warn_worktree_container_gitdir(&project_root, &wt_path);
    aida_core::git_ops::init_submodules_or_warn(
        &wt_path,
        crate::worktree_config_init_submodules(&project_root),
    )
    .with_context(|| format!("prepare submodules in PR worktree {}", wt_path.display()))?;

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
            // trace:BUG-1622 | ai:claude
            .args([
                "merge-base",
                "--is-ancestor",
                crate::git_arg_guard::END_OF_OPTIONS,
                parent_sha,
                "HEAD",
            ])
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
            // trace:BUG-1622 | ai:claude
            Some(parent_sha) => cmd.args([
                "rebase",
                "--onto",
                &origin_base,
                crate::git_arg_guard::END_OF_OPTIONS,
                parent_sha,
            ]),
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
            // trace:BUG-1985 | ai:claude
            let status_says_rebasing = status_output_says_rebasing(
                aida_core::git_ops::host_git_command(&wt_path)
                    .args(["status", "--porcelain=v2", "--branch"])
                    .output(),
            );
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
            // trace:BUG-1295 | ai:claude — typed exit code, not prose, is
            // what `attempt_phase3_auto_rebase` classifies on.
            return Err(pr_rebase::rebase_conflict_error(
                "rebase aborted due to conflicts",
            ));
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
            if !matches!(mode, PrRebaseMode::Interactive) {
                cleanup_worktree();
            }
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
            // trace:BUG-2181 | ai:claude
            .remote_credentials(wt_path, remote)
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
            let detail = pr_rebase::force_push_block_message(&info.head_ref, &commits);
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                detail
            );
            // BUG-1218: an autonomous retry must not inherit a checked-out
            // pr-N branch. The full guard recipe above is the durable diagnostic.
            preserve_pr_rebase_diagnostic(&project_root, n, &detail);
            cleanup_worktree();
            // trace:BUG-1295 | ai:claude — typed exit code, not prose.
            return Err(pr_rebase::rebase_refused_error(
                "force-push refused: remote has un-incorporated commits",
            ));
        }
        pr_rebase::ForcePushGuard::Inconclusive { reason } => {
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                pr_rebase::force_push_inconclusive_message(&info.head_ref, &reason)
            );
            cleanup_worktree();
            // trace:BUG-1295 | ai:claude — typed exit code, not prose.
            return Err(pr_rebase::rebase_refused_error(
                "force-push refused: could not verify remote (fail-closed)",
            ));
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
        // trace:BUG-2181 | ai:claude
        .remote_credentials(&wt_path, "origin")
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
        if !matches!(mode, PrRebaseMode::Interactive) {
            cleanup_worktree();
        }
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
    fetch_change_info_via_resolved_forge(
        project_root,
        n,
        crate::forge::resolve_forge_kind(project_root),
    )
}

// trace:BUG-1037 | ai:codex
pub(crate) fn fetch_change_info_via_resolved_forge(
    project_root: &std::path::Path,
    n: u64,
    forge_kind: crate::forge::ForgeKind,
) -> Result<pr_rebase::PrInfo> {
    let noun = forge_kind.change_noun();
    let cli = forge_kind.cli_name();
    let m = crate::forge::forge_for_kind(project_root, forge_kind)
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
    let mut command = std::process::Command::new(gh_bin);
    command.current_dir(project_root).args([
        "pr",
        "view",
        n_str.as_str(),
        "--json",
        "baseRefName,headRefName,headRefOid,isCrossRepository,headRepository,isDraft",
    ]);
    // trace:BUG-1689 | ai:codex
    let out = crate::process_retry::command_output_retrying_etxtbsy(&mut command).map_err(|e| {
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
/// as `aida` subcommands (resolved via the hardened executable resolver) so the wrapper
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
    let mut ids: Vec<String> = Vec::new();
    for (_sha, subject) in &commits {
        ids.extend(pr_ship::extract_trailing_spec_ids_from_subject(subject));
    }
    ship_gate_spec_records_for_ids(project_root, ids)
}

/// [`ship_gate_spec_records`] for an explicit set of spec IDs: what
/// `aida pr train` reads for a member PR, whose commits are not the local
/// range.
// trace:STORY-1690 | ai:claude
pub(crate) fn ship_gate_spec_records_for_ids(
    project_root: &std::path::Path,
    mut ids: Vec<String>,
) -> Vec<ShipGateSpecRecord> {
    let Some(store) = load_store_for_lookup(project_root) else {
        return Vec::new();
    };
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
    draft_only_specs_in(&ship_gate_spec_records(project_root))
}

/// The `review:draft-only` filter over already-read spec records.
// trace:STORY-529 trace:STORY-1690 | ai:claude
pub(crate) fn draft_only_specs_in(records: &[ShipGateSpecRecord]) -> Vec<String> {
    records
        .iter()
        .filter(|(_, tags, _)| pr_ship::is_draft_only_tagged(tags))
        .map(|(id, _, _)| id.clone())
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
    supervised_specs_in(ship_gate_spec_records(project_root), interactive_tty)
}

/// The BUG-727 supervised-merge filter over already-read spec records.
// trace:BUG-727 trace:STORY-1690 | ai:claude
pub(crate) fn supervised_specs_in(
    records: Vec<ShipGateSpecRecord>,
    interactive_tty: bool,
) -> Vec<(String, String)> {
    // STORY-1698: every spec read during a ship feeds the mandate's
    // append-only exclusion ledger. trace:STORY-1698 | ai:claude
    if let Ok(dir) = crate::mandate::mandate_dir() {
        for (id, tags, mode) in &records {
            let mut req = aida_core::Requirement::new(String::new(), String::new());
            req.tags = tags.iter().cloned().collect();
            req.execution_mode = *mode;
            crate::mandate::note_exclusions(&dir, id, [&req]);
        }
    }
    let specs: Vec<(String, Option<aida_core::ExecutionMode>)> = records
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
    // `aida ship` merges unless told to stop at the PR; refuse that before
    // any commit or push. `--no-merge` / `--no-pr` stay open to a standalone
    // implementer. trace:STORY-1662 | ai:claude
    if !opts.no_merge && !opts.no_pr {
        crate::standalone_implementer::refuse_under_marker("ship")?;
    }

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
    let remote_branch = origin_ref
        .strip_prefix("origin/")
        .unwrap_or(origin_ref.as_str())
        .to_string();
    // The base comes from the forge (a PR's baseRefName); never let it
    // read as a git option. trace:BUG-1622 | ai:claude
    crate::git_arg_guard::reject_option_like("PR base branch", &remote_branch)?;
    eprintln!("  step 2: rebasing onto current {origin_ref}");
    let fetch = std::process::Command::new("git")
        .current_dir(&project_root)
        .args([
            "fetch",
            crate::git_arg_guard::END_OF_OPTIONS,
            "origin",
            &remote_branch,
        ])
        // trace:BUG-2181 | ai:claude
        .remote_credentials(&project_root, "origin")
        .status()
        .context("could not invoke `git fetch`")?;
    if !fetch.success() {
        anyhow::bail!("`git fetch origin {remote_branch}` failed — is the remote reachable?");
    }
    let rebase = std::process::Command::new("git")
        .current_dir(&project_root)
        // trace:BUG-1622 | ai:claude
        .args(["rebase", crate::git_arg_guard::END_OF_OPTIONS, &origin_ref])
        .status()
        .context("could not invoke `git rebase`")?;
    if !rebase.success() {
        // Abort so the worktree is left clean, then bail with the manual recipe.
        let _ = std::process::Command::new("git")
            .current_dir(&project_root)
            .args(["rebase", "--abort"])
            .status();
        anyhow::bail!(
            "rebase onto {origin_ref} hit conflicts — aborted, worktree left clean.\n  \
             Resolve by hand: `git rebase {origin_ref}`, fix the conflicts, \
             `git rebase --continue`, then re-run `aida ship`."
        );
    }
    eprintln!(
        "  {} rebased onto {origin_ref}",
        crate::glyph(crate::glyphs::Glyph::Check).green()
    );

    // ---- Step 3: push (force-with-lease — the rebase may have rewritten history). ----
    eprintln!("  step 3: pushing {} to origin", branch);
    // trace:TASK-2270 | ai:claude
    aida_core::git_ops::clear_sandbox_config_lock(&project_root);
    let push = std::process::Command::new("git")
        .current_dir(&project_root)
        .args(["push", "--force-with-lease", "-u", "origin", &branch])
        // trace:BUG-2181 | ai:claude
        .remote_credentials(&project_root, "origin")
        .status()
        .context("could not invoke `git push`")?;
    if !push.success() {
        anyhow::bail!(
            "`git push --force-with-lease -u origin {branch}` failed — investigate before retrying"
        );
    }
    aida_core::git_ops::repair_push_upstream(&project_root, "origin", Some(&branch), &branch);
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
                false,                 // override_stale_check — not exposed on `aida ship` yet
                false, // override_stale_approval — not exposed on `aida ship` (TASK-1448)
                None,  // wait — not exposed on `aida ship`
                false, // no_complete — not exposed on `aida ship` (BUG-1919)
            )
        }
    }
}

// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
/// Resolve the PR's base (target) branch for the merge-lease key, falling back
/// to `main`. A per-target-branch lease serializes only mergers heading to the
/// same branch.
// trace:STORY-1171 | ai:claude
pub(crate) fn pr_ship_target_branch(pr: u64) -> String {
    finish_ceremony_pr_base(pr).unwrap_or_else(|| "main".to_string())
}

/// Read PR `pr`'s real base branch (`baseRefName`) from the forge. Returns
/// `None` — never a guessed default — when `gh` is missing, the call fails,
/// or the field comes back empty; callers decide the fallback and whether
/// to say so.
// trace:TASK-1416 | ai:claude
fn finish_ceremony_pr_base(pr: u64) -> Option<String> {
    let mut command = std::process::Command::new("gh");
    command.args([
        "pr",
        "view",
        &pr.to_string(),
        "--json",
        "baseRefName",
        "-q",
        ".baseRefName",
    ]);
    crate::process_retry::command_output_retrying_etxtbsy(&mut command)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Acquire the branch merge-lease for this ship, or REFUSE (Err) if a live merger
/// outlasts the bounded wait. The returned guard releases on drop.
// trace:STORY-1171 | ai:claude
fn acquire_merge_lease(
    main_worktree: &std::path::Path,
    pr: u64,
) -> Result<crate::merge_lock::MergeLease> {
    let target = pr_ship_target_branch(pr);
    crate::merge_lock::acquire(
        main_worktree,
        &target,
        Some(pr),
        "aida pr ship",
        crate::merge_lock::DEFAULT_WAIT,
    )
    .map_err(|e| {
        anyhow::anyhow!(
            "another merger holds the '{target}' merge-lease: {e} \
             (a concurrent merge is in progress — retry once it releases, or run `aida merge-lock`)"
        )
    })
}

// Emit the same `PrMerged` event the drain's merge phase emits
// (`DriveContext::merge_pr` in `lib.rs`), from `aida pr ship` — so a feed
// consumer cannot tell whether a merge came from the orchestrator or an
// explicit ship. One event per spec the PR is credited to; a PR crediting no
// resolvable spec still emits once with `spec: None` so the merge is never
// silently dropped. `emit` itself is best-effort (never errors the ship);
// isolated here so the target-derivation logic is unit-testable without
// stubbing `gh`. trace:BUG-1423 | ai:claude
pub(crate) fn emit_ship_pr_merged(main_worktree: &std::path::Path, pr: u32, spec_ids: &[String]) {
    let seat = crate::events::active_seat();
    let targets: Vec<Option<String>> = if spec_ids.is_empty() {
        vec![None]
    } else {
        spec_ids.iter().cloned().map(Some).collect()
    };
    for spec in targets {
        let mut ev = crate::events::Event::new(spec, "", crate::events::EventKind::PrMerged { pr });
        ev.seat = seat.clone();
        crate::events::emit(main_worktree, &ev);
    }
}

/// TASK-1448: the PR's CURRENT head sha for the approval-covers-head gate,
/// read from the forge (gh `headRefOid`; pure-git resolves `branch`).
/// `None` when it cannot be read — which the gate treats as "cannot show
/// the approval covers the head", i.e. refuse (PRIN-5).
// trace:TASK-1448 | ai:claude
fn pr_head_sha_for_merge_gate(
    project_root: &std::path::Path,
    pr_number: u64,
    branch: &str,
) -> Option<String> {
    let change = crate::forge::ChangeRef {
        id: pr_number,
        url: String::new(),
        branch: branch.to_string(),
        base: String::new(),
        title: None,
    };
    crate::forge::forge_for(project_root)
        .change_status(&change)
        .ok()
        .map(|status| status.head_sha.trim().to_string())
        .filter(|sha| !sha.is_empty())
}

/// `aida pr ship` released the marker but could not drop the forge hold
/// label. The marker is gone, so the label is now a label-only hold: name
/// the human clear that drops it and the ship re-run, not a bare
/// `merge-hold clear` retry.
// trace:TASK-1951 | ai:claude
pub(crate) fn ship_unlabel_failure(pr: u64, err: &crate::merge_hold::LabelSyncError) -> String {
    format!(
        "could not remove the merge-hold label ({err}) — branch protection will keep the merge closed. Drop it with `aida merge-hold clear {pr}` when the forge is reachable, then re-run `aida pr ship {pr}`."
    )
}

/// BUG-1532 criterion 4: does a fresh verdict at `head` release this refusal
/// hold? Verdicts are read under every root (the main clone and the calling
/// worktree can both hold `.aida/review-verdicts/`); the first root with an
/// answer wins. Never clears anything.
// trace:BUG-1532 | ai:claude
pub(crate) fn ship_refusal_release(
    roots: &[&std::path::Path],
    record: &crate::merge_hold::MergeHoldRecord,
    head: Option<&str>,
) -> crate::merge_hold::RefusalRelease {
    crate::merge_hold::refusal_release(
        record,
        head,
        |key| {
            roots
                .iter()
                .find_map(|root| crate::review_verdict::read_recorded_verdict(root, key))
        },
        |key, sha| {
            roots
                .iter()
                .find_map(|root| crate::review_verdict::read_verdict_for_sha(root, key, sha))
        },
    )
}

/// What the approval gate decides once approval AUTHORITY holds.
// trace:TASK-1945 | ai:claude
#[derive(Debug)]
pub(crate) enum HeadDecision {
    /// An approval covers the head: merge pinned to this (`None`: nothing to
    /// pin to).
    Covered(Option<String>),
    /// TASK-1960: the head moved only by clean merges of the base branch,
    /// with the PR's own diff unchanged, so the attested approval carries
    /// over to `proof.new_head`. The outcome is `None` for a quiet site.
    CarriedOver(
        crate::review_carryover::CarryOver,
        Option<crate::review_carryover::RecordOutcome>,
    ),
    /// No approval covers the head; `message` says why, including a refused
    /// carry-over. Only `--override-stale-approval` can pass it.
    Stale {
        refusal: pr_ship::ApprovalHeadRefusal,
        message: String,
    },
}

/// The approval gate's decision with no output, log or event of its own:
/// `Err(message)` when no authorized, non-author approval exists (BUG-1918,
/// never overridable), else whether it covers `head_sha`. A carry-over is
/// tried through `carry_over`; a quiet site records nothing, so through one
/// this reads only. `aida pr ship` and the review watcher's readiness report
/// both decide here, so the two cannot disagree.
// trace:TASK-1945 | ai:claude
pub(crate) fn approval_decision(
    pr_number: u64,
    candidates: &[crate::review_verdict::RecordedVerdict],
    head_sha: Option<&str>,
    policy: &pr_ship::ShipApprovalPolicy,
    carry_over: Option<&crate::review_carryover::CarryOverSite<'_>>,
) -> std::result::Result<HeadDecision, String> {
    // BUG-1918: the approval must come from an authorized, non-author seat.
    // Checked first: `--override-stale-approval` excuses a head that moved
    // past a real approval, never a missing or self-recorded one.
    // trace:BUG-1918 | ai:claude
    if let Some(refusal) = pr_ship::approval_authority_refusal(candidates, policy) {
        return Err(pr_ship::approval_authority_refusal_message(
            pr_number, &refusal,
        ));
    }
    let Some(refusal) = pr_ship::approval_head_refusal(candidates, head_sha) else {
        return Ok(HeadDecision::Covered(pr_ship::approved_match_head(
            candidates, head_sha,
        )));
    };
    let mut message = pr_ship::approval_head_refusal_message(pr_number, &refusal);
    // TASK-1960: a head that moved only by clean merges of the base branch,
    // with the PR's own diff unchanged, keeps the attested approval. The
    // merge is pinned to the proven head, so a later push still refuses.
    // trace:TASK-1960 | ai:claude
    match carry_over
        .and_then(|site| crate::review_carryover::carry_over_for_refusal(site, pr_number, &refusal))
    {
        Some(Ok((proof, outcome))) => return Ok(HeadDecision::CarriedOver(proof, outcome)),
        Some(Err(why)) => {
            message = format!(
                "{message} The approval did not carry over: {}.",
                why.reason()
            );
        }
        None => {}
    }
    Ok(HeadDecision::Stale { refusal, message })
}

/// TASK-1448 + TASK-1458: `aida pr ship`'s approval-covers-head gate.
/// `Ok(pin)` = merge, pinned to `pin` (`MergeOptions.match_head`, i.e. gh
/// `--match-head-commit`) so a push between this check and the merge is
/// refused by the forge instead of landing unreviewed. The pin is the
/// approved head when an open approval covers it, `None` when there is no
/// approval, and — under `--override-stale-approval` — the head the override
/// was granted for, after a durable `advisor-activity.jsonl` record naming
/// both shas. `Err` = refused (logged as a failed merge step).
// trace:TASK-1458 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn pr_ship_approval_gate(
    main_worktree: &std::path::Path,
    pr_number: u64,
    candidates: &[crate::review_verdict::RecordedVerdict],
    head_sha: Option<&str>,
    policy: &pr_ship::ShipApprovalPolicy,
    override_stale_approval: bool,
    delete_branch: bool,
    carry_over: Option<&crate::review_carryover::CarryOverSite<'_>>,
) -> Result<Option<String>> {
    let decision = match approval_decision(pr_number, candidates, head_sha, policy, carry_over) {
        Ok(decision) => decision,
        Err(message) => {
            log_ship_activity(
                main_worktree,
                Some(pr_number),
                &pr_ship::ShipStep::Merge { delete_branch },
                &pr_ship::StepOutcome::Failed(message.clone()),
            );
            crate::events::record_gate_held(
                main_worktree,
                crate::events::GATE_APPROVAL_AUTHORITY,
                None,
                Some(pr_number),
                &message,
            );
            return Err(pr_ship::ShipRefusal::Review.error(message));
        }
    };
    // STORY-1655: name the mode when a solo-human approval is relied on.
    // trace:STORY-1655 | ai:claude
    // Same stream as merge_path_approval_gate. trace:TASK-1955 | ai:claude (TASK-1808)
    if let Some(notice) = pr_ship::solo_approval_notice(candidates, policy) {
        eprintln!("  {notice}");
    }
    let (refusal, message) = match decision {
        HeadDecision::Covered(pin) => return Ok(pin),
        HeadDecision::CarriedOver(proof, outcome) => {
            // A quiet (preflight) site records nothing and stays silent; the
            // gate that pins the merge announces it once.
            if let Some(outcome) = outcome {
                println!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    crate::review_carryover::carry_over_line(&proof, &outcome)
                );
                append_ship_activity_line(
                    main_worktree,
                    &crate::review_carryover::carry_over_event(
                        &chrono::Utc::now().to_rfc3339(),
                        "aida pr ship",
                        &proof,
                    ),
                );
            }
            return Ok(Some(proof.new_head));
        }
        HeadDecision::Stale { refusal, message } => (refusal, message),
    };
    // BUG-1918: only a non-author session may ship commits no approval
    // covered; the override is neither honoured nor advertised to an author.
    // trace:BUG-1918 | ai:claude
    let override_refusal = pr_ship::stale_override_refusal(policy);
    if !override_stale_approval || override_refusal.is_some() {
        let message = match (&override_refusal, override_stale_approval) {
            (Some(why), true) => {
                format!("{message} `--override-stale-approval` refused: {why}.")
            }
            _ => message,
        };
        log_ship_activity(
            main_worktree,
            Some(pr_number),
            &pr_ship::ShipStep::Merge { delete_branch },
            &pr_ship::StepOutcome::Failed(message.clone()),
        );
        // STORY-1436: the stale-approval gate HELD. trace:STORY-1436 | ai:claude
        crate::events::record_gate_held(
            main_worktree,
            crate::events::GATE_STALE_APPROVAL,
            None,
            Some(pr_number),
            &message,
        );
        return Err(pr_ship::ShipRefusal::Review.error(match override_refusal {
            None => {
                format!("{message} (To merge anyway, re-run with `--override-stale-approval`.)")
            }
            Some(_) => message,
        }));
    }
    append_ship_activity_line(
        main_worktree,
        &pr_ship::format_stale_approval_override_event(
            &chrono::Utc::now().to_rfc3339(),
            pr_number,
            &refusal,
        ),
    );
    eprintln!(
        "  {} {message} — shipping anyway (--override-stale-approval)",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
    );
    Ok(head_sha
        .map(str::trim)
        .filter(|sha| !sha.is_empty())
        .map(str::to_string))
}

/// BUG-1918: what PR `pr` ships, for the approval gate — `known_ids` (from
/// the local range, branch, title) plus what the FORGE says: the spec ids in
/// the PR's head ref and in the trailers of every commit `base..head` adds.
/// The head branch returned is the forge's head ref when known. Resolved
/// from the forge so shipping from `main` (where the local range is empty)
/// or under a renamed local branch still finds what the PR ships. The third
/// value says whether the forge answered: when it did not, the ids are only
/// what the local title/branch claim, so the caller must judge authorship
/// against EVERY author (fail closed), using the ids only to find verdicts.
// trace:BUG-1918 | ai:claude
pub(crate) fn pr_gate_subject(
    main_worktree: &std::path::Path,
    pr: u64,
    known_ids: &[String],
    head_branch: Option<&str>,
) -> (Vec<String>, Option<String>, bool, Vec<String>) {
    let mut ids = known_ids.to_vec();
    // `(Refs X)` subjects name the work's spec for authorship only, never
    // for verdict lookup or completion. trace:BUG-2199 | ai:claude
    let mut refs_ids: Vec<String> = Vec::new();
    let mut forge_resolved = false;
    let mut branch = head_branch
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string);
    if let Some(b) = &branch {
        ids.extend(pr_ship::extract_spec_ids_from_text(b));
    }
    match crate::review_pr_subject_from_forge(main_worktree, pr) {
        Ok(subject) => {
            forge_resolved = true;
            let head = subject.head_ref.trim();
            if !head.is_empty() {
                ids.extend(pr_ship::extract_spec_ids_from_text(head));
                branch = Some(head.to_string());
            }
            for s in &subject.commit_subjects {
                ids.extend(pr_ship::extract_trailing_spec_ids_from_subject(s));
                refs_ids.extend(pr_ship::extract_authorship_spec_ids_from_subject(s));
            }
        }
        Err(e) => eprintln!(
            "  warning: could not read PR-{pr}'s head and commits from the forge ({e:#}); \
             every recorded author is treated as one of its authors"
        ),
    }
    refs_ids.extend(ids.iter().cloned());
    (
        normalized_spec_ids(&ids),
        branch,
        forge_resolved,
        normalized_spec_ids(&refs_ids),
    )
}

/// The spec IDs the ship approval gate looks verdicts up under: the gate
/// records for the local range, then every ID in the branch and the title.
/// `aida pr ship-batch` waits on the same set, so its "approved at head"
/// cannot drift from what ship accepts.
// trace:STORY-1887 | ai:claude
pub(crate) fn ship_gate_known_ids(
    project_root: &std::path::Path,
    branch: &str,
    title: Option<&str>,
) -> Vec<String> {
    let mut ids: Vec<String> = ship_gate_spec_records(project_root)
        .into_iter()
        .map(|(id, _, _)| id)
        .collect();
    ids.extend(pr_ship::extract_spec_ids_from_text(branch));
    if let Some(title) = title {
        ids.extend(pr_ship::extract_spec_ids_from_text(title));
    }
    ids
}

/// BUG-1918: the approval candidates for merging PR `pr` (read from `roots`)
/// and the [`ShipApprovalPolicy`] they are judged by — the one input every
/// merge path (`aida pr ship`, the drain, `aida release --after-pr`,
/// burndown) builds the same way.
// trace:BUG-1918 | ai:claude
pub(crate) fn merge_approval_inputs(
    roots: &[&std::path::Path],
    main_worktree: &std::path::Path,
    pr: u64,
    known_ids: &[String],
    head_branch: Option<&str>,
    head_sha: Option<&str>,
) -> (
    Vec<crate::review_verdict::RecordedVerdict>,
    pr_ship::ShipApprovalPolicy,
) {
    let (candidates, policy, _) =
        merge_approval_subject(roots, main_worktree, pr, known_ids, head_branch, head_sha);
    (candidates, policy)
}

/// [`merge_approval_inputs`] plus the spec ids the PR ships, which the
/// corpus gate takes as its hints.
// trace:TASK-1945 | ai:claude
pub(crate) fn merge_approval_subject(
    roots: &[&std::path::Path],
    main_worktree: &std::path::Path,
    pr: u64,
    known_ids: &[String],
    head_branch: Option<&str>,
    head_sha: Option<&str>,
) -> (
    Vec<crate::review_verdict::RecordedVerdict>,
    pr_ship::ShipApprovalPolicy,
    Vec<String>,
) {
    let (ids, branch, forge_resolved, authorship_ids) =
        pr_gate_subject(main_worktree, pr, known_ids, head_branch);
    let candidates = pr_ship::merge_gate_verdict_candidates(roots, pr, &ids, head_sha);
    // An unconfirmed subject (forge unreachable) names no author set: every
    // recorded author counts. trace:BUG-1918 | ai:claude
    // trace:BUG-2199 | ai:claude
    let author_ids: &[String] = if forge_resolved { &authorship_ids } else { &[] };
    let policy = ship_approval_policy(main_worktree, author_ids, branch.as_deref(), &candidates);
    (candidates, policy, ids)
}

/// BUG-2184: what is left of `--wait SECS` for the CI wait: the time since the
/// wait started, less the mandate scan's own time.
// trace:BUG-2184 | ai:claude
pub(crate) fn ship_ci_budget(
    secs: u64,
    since_wait_started: std::time::Duration,
    scan_spent: std::time::Duration,
) -> std::time::Duration {
    std::time::Duration::from_secs(secs)
        .saturating_sub(since_wait_started.saturating_sub(scan_spent))
}

// STORY-1700 (C1): the mandate ship cover lives in `mandate_ship.rs`.
// trace:STORY-1700 | ai:claude
pub(crate) use crate::mandate_ship::{
    mandate_carry_over, mandate_exclusion_specs, mandate_ship_cover, mandate_still_active,
    rearm_after_mandate_failure, rearm_mandate_hold_with, release_hold_under_mandate, MandateCover,
};

/// BUG-1918: the approval gate for merge paths other than `aida pr ship`
/// (`aida release --after-pr`, burndown): approval AUTHORITY, then head
/// coverage, with no override. `Ok(pin)` = merge pinned to `pin`
/// (`MergeOptions.match_head`); `Err` = the refusal text, already recorded
/// as a held gate.
// trace:BUG-1918 | ai:claude
pub(crate) fn merge_path_approval_gate(
    project_root: &std::path::Path,
    pr: u64,
    known_ids: &[String],
    head_branch: Option<&str>,
    head_sha: Option<&str>,
) -> std::result::Result<Option<String>, String> {
    let main_worktree = crate::main_worktree_root_from(project_root);
    let (candidates, policy) = merge_approval_inputs(
        &[project_root, main_worktree.as_path()],
        &main_worktree,
        pr,
        known_ids,
        head_branch,
        head_sha,
    );
    // TASK-1960: the base is read from the forge (prove time and again right
    // before the caller merges). trace:TASK-1960 | ai:claude
    let site = crate::review_carryover::CarryOverSite::new(&main_worktree, None);
    merge_path_approval_decision(
        &main_worktree,
        pr,
        &candidates,
        &policy,
        head_sha,
        &site,
        &|| {
            crate::forge::forge_for(project_root)
                .change_metadata(pr, &mut crate::network_retry::NoopSink)
                .ok()
                .map(|m| m.base_ref)
        },
    )
}

/// [`merge_path_approval_gate`] once its inputs are gathered: authority,
/// then head coverage with the TASK-1960 carry-over through `site`, whose
/// proven base must still match `base_now()` (the PR's base at merge time).
// trace:BUG-1918 | ai:claude
// trace:TASK-1960 | ai:claude
pub(crate) fn merge_path_approval_decision(
    main_worktree: &std::path::Path,
    pr: u64,
    candidates: &[crate::review_verdict::RecordedVerdict],
    policy: &pr_ship::ShipApprovalPolicy,
    head_sha: Option<&str>,
    site: &crate::review_carryover::CarryOverSite<'_>,
    base_now: &dyn Fn() -> Option<String>,
) -> std::result::Result<Option<String>, String> {
    if let Some(refusal) = pr_ship::approval_authority_refusal(candidates, policy) {
        let message = pr_ship::approval_authority_refusal_message(pr, &refusal);
        crate::events::record_gate_held(
            main_worktree,
            crate::events::GATE_APPROVAL_AUTHORITY,
            None,
            Some(pr),
            &message,
        );
        return Err(message);
    }
    // STORY-1655: name the mode when a solo-human approval is relied on.
    // trace:STORY-1655 | ai:claude
    if let Some(notice) = pr_ship::solo_approval_notice(candidates, policy) {
        eprintln!("  {notice}");
    }
    if let Some(refusal) = pr_ship::approval_head_refusal(candidates, head_sha) {
        let mut message = pr_ship::approval_head_refusal_message(pr, &refusal);
        // TASK-1960: the same carry-over `aida pr ship` applies.
        // trace:TASK-1960 | ai:claude
        match crate::review_carryover::carry_over_for_refusal(site, pr, &refusal) {
            Some(Ok((proof, outcome))) => {
                // Every caller merges right after this gate returns, so the
                // PR's base is re-read here: a base retargeted since the
                // proof refuses. trace:TASK-1960 | ai:claude
                if let Some(why) = crate::review_carryover::proven_base_refusal(
                    pr,
                    &proof.base_branch(),
                    base_now().as_deref(),
                ) {
                    crate::events::record_gate_held(
                        main_worktree,
                        crate::events::GATE_STALE_APPROVAL,
                        None,
                        Some(pr),
                        &why,
                    );
                    return Err(why);
                }
                if let Some(outcome) = &outcome {
                    eprintln!(
                        "  {}",
                        crate::review_carryover::carry_over_line(&proof, outcome)
                    );
                }
                append_ship_activity_line(
                    main_worktree,
                    &crate::review_carryover::carry_over_event(
                        &chrono::Utc::now().to_rfc3339(),
                        "merge",
                        &proof,
                    ),
                );
                return Ok(Some(proof.new_head));
            }
            Some(Err(why)) => {
                message = format!(
                    "{message} The approval did not carry over: {}.",
                    why.reason()
                );
            }
            None => {}
        }
        crate::events::record_gate_held(
            main_worktree,
            crate::events::GATE_STALE_APPROVAL,
            None,
            Some(pr),
            &message,
        );
        return Err(message);
    }
    Ok(pr_ship::approved_match_head(candidates, head_sha))
}

/// BUG-1918: the approval policy for the specs a PR ships — its author
/// sessions (authoring leases on those specs or on the PR's head branch, live
/// or recorded in the durable authorship ledger; with NO spec id, every live
/// authoring lease and ledger record, failing closed), which recorded
/// approvals' authority re-validates against its durable store, and the
/// committer date of the commit the newest approval reviewed. There is no
/// review opt-out. A missing or malformed ledger is warned about.
// trace:BUG-1918 | ai:claude
pub(crate) fn ship_approval_policy(
    main_worktree: &std::path::Path,
    spec_ids: &[String],
    head_branch: Option<&str>,
    candidates: &[crate::review_verdict::RecordedVerdict],
) -> pr_ship::ShipApprovalPolicy {
    let ids = normalized_spec_ids(spec_ids);
    crate::review_authority::warn_on_ledger_read(main_worktree);
    let author_sessions = if ids.is_empty() {
        crate::review_authority::all_author_sessions(main_worktree, head_branch)
    } else {
        crate::review_authority::author_sessions_at(main_worktree, &ids, head_branch)
    };
    let approvals: Vec<crate::review_verdict::RecordedVerdict> = candidates
        .iter()
        .filter(|v| v.kind.approves() && !v.is_closed())
        .cloned()
        .collect();
    let reviewed_committed_at = pr_ship::newest_approval(&approvals)
        .and_then(|a| a.reviewed_sha.as_deref())
        .and_then(|sha| commit_committed_at_fetching(main_worktree, sha));
    pr_ship::ShipApprovalPolicy {
        author_sessions,
        verified_authority: crate::review_authority::verified_authority_keys(
            main_worktree,
            candidates,
        ),
        reviewed_committed_at,
        // BUG-1918: who is merging — `--override-stale-approval` is refused
        // to an author. trace:BUG-1918 | ai:claude
        shipper: crate::review_authority::RecorderIdentity::current(
            &crate::review_authority::visible_leases(main_worktree),
        )
        .tokens,
        // STORY-1655: the solo mode is re-read at ship time, never trusted
        // from the verdict. trace:STORY-1655 | ai:claude
        solo_human: crate::solo_review::effective(main_worktree),
        verified_solo: crate::review_authority::verified_solo_keys(candidates),
    }
}

/// [`commit_committed_at`], fetching a full sha from `origin` once when it is
/// not present locally (a reviewed commit since force-pushed away).
// trace:BUG-1918 | ai:claude
fn commit_committed_at_fetching(
    repo: &std::path::Path,
    sha: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    commit_committed_at(repo, sha).or_else(|| {
        let sha = sha.trim();
        if sha.len() != 40 || !sha.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let fetched = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["fetch", "--quiet", "--no-tags", "origin", sha])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // trace:BUG-2181 | ai:claude
            .remote_credentials(repo, "origin")
            .status()
            .is_ok_and(|s| s.success());
        fetched.then(|| commit_committed_at(repo, sha)).flatten()
    })
}

/// BUG-1918: the committer date of `sha` in `repo`, when the commit is
/// present locally.
// trace:BUG-1918 | ai:claude
pub(crate) fn commit_committed_at(
    repo: &std::path::Path,
    sha: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let sha = sha.trim();
    if sha.is_empty() || sha.starts_with('-') {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "-1", "--format=%cI", sha, "--"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    chrono::DateTime::parse_from_rfc3339(String::from_utf8_lossy(&out.stdout).trim())
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// Trimmed, upper-cased, deduplicated, non-empty spec ids.
// trace:BUG-1918 | ai:claude
pub(crate) fn normalized_spec_ids(spec_ids: &[String]) -> Vec<String> {
    let mut ids: Vec<String> = spec_ids
        .iter()
        .map(|id| id.trim().to_ascii_uppercase())
        .filter(|id| !id.is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// The stale-definition gate's own predicate: PR `pr_number`'s base branch
/// and the staleness warning its ship branch carries against it. A warning
/// with `definition_files` is exactly what [`ship_behind_base_gate`]
/// refuses, so `aida pr ship-batch` reads this, never a re-implementation.
// trace:STORY-1887 | ai:claude
pub(crate) fn ship_stale_definition_warning(
    project_root: &std::path::Path,
    ship_branch: &str,
    pr_number: u64,
) -> (String, Option<StaleCheckWarning>) {
    let base_branch = pr_ship_target_branch(pr_number);
    let base_ref = format!("origin/{base_branch}");
    let warning = pr_stale_check_warning(project_root, ship_branch, &base_ref);
    (base_branch, warning)
}

/// What the stale gate does with PR `pr_number`'s staleness against
/// `base_branch`: `Ok` ships, `Err` is the refusal. TASK-1606 refused only a
/// green that predates a CI-definition change. STORY-1690: a head missing
/// ANY commit on the base refuses, and no flag overrides it — two PRs each
/// green on its own base can break main together (#2599/#2597), so a behind
/// head lands through `aida pr train N`, which tests it on the current base.
// trace:TASK-1606 | ai:codex
// trace:STORY-1690 | ai:claude
pub(crate) fn stale_gate_decision(
    pr_number: u64,
    base_branch: &str,
    warning: Option<&StaleCheckWarning>,
    override_stale_check: bool,
) -> std::result::Result<(), String> {
    let Some(warning) = warning else {
        return Ok(());
    };
    let definition = if warning.definition_files.is_empty() {
        String::new()
    } else {
        format!(
            " Its CI also ran against an OLDER definition of the check ({} changed since).",
            warning.definition_files.join(", ")
        )
    };
    let flag = if override_stale_check {
        " `--override-stale-check` does not ship a head that is behind its base."
    } else {
        ""
    };
    Err(format!(
        "PR-{pr_number}'s head is {} commit(s) behind {base_branch}, so its green was never \
         tested together with them; two PRs each green on its own base can still turn \
         {base_branch} red together.{definition} Land it with `aida pr train {pr_number}` \
         (it tests the PR on top of the current {base_branch} before merging), or merge \
         {base_branch} into the branch, wait for green, and ship again.{flag}",
        warning.behind_commits,
    ))
}

/// STORY-1690: fetch `base_branch` and the PR head `ship_branch` from origin,
/// so the behind-base gate measures against the base as the forge has it
/// now, not a local ref another machine, a web merge or another seat has
/// since moved past.
// trace:STORY-1690 | ai:claude
pub(crate) fn fetch_ship_refs(
    project_root: &std::path::Path,
    base_branch: &str,
    ship_branch: &str,
) -> std::result::Result<(), String> {
    let mut refspecs = vec![format!(
        "+refs/heads/{base_branch}:refs/remotes/origin/{base_branch}"
    )];
    if ship_branch != base_branch {
        refspecs.push(format!(
            "+refs/heads/{ship_branch}:refs/remotes/origin/{ship_branch}"
        ));
    }
    // TASK-2143: through the host-git chokepoint, which refuses a seat clone.
    let out = aida_core::git_ops::host_git_command(project_root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["fetch", "--quiet", "--no-tags", "origin"])
        .args(&refspecs)
        // trace:TASK-1952 | ai:claude
        .remote_credentials(project_root, "origin")
        .output()
        .map_err(|e| format!("git fetch: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git fetch origin {base_branch} {ship_branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// The behind-base gate's outcome for PR `pr_number`: `staleness` is the
/// fail-closed [`crate::pr_stale_check`] result (or the fetch error). A head
/// behind its base refuses as `BehindBase`, or `StaleDefinition` when a CI
/// definition also changed; staleness that cannot be computed refuses too.
// trace:STORY-1690 | ai:claude
pub(crate) fn behind_base_refusal(
    pr_number: u64,
    base_branch: &str,
    staleness: std::result::Result<Option<StaleCheckWarning>, String>,
    override_stale_check: bool,
) -> std::result::Result<(), (pr_ship::ShipRefusal, String)> {
    use pr_ship::ShipRefusal;
    let warning = staleness.map_err(|e| {
        (
            ShipRefusal::BehindBase,
            format!(
                "PR-{pr_number}: whether its head is behind {base_branch} could not be \
                 computed ({e}), so its green may be stale; refusing rather than shipping it. \
                 Fix the fetch, or land it with `aida pr train {pr_number}`."
            ),
        )
    })?;
    let kind = if warning
        .as_ref()
        .is_some_and(|w| !w.definition_files.is_empty())
    {
        ShipRefusal::StaleDefinition
    } else {
        ShipRefusal::BehindBase
    };
    stale_gate_decision(
        pr_number,
        base_branch,
        warning.as_ref(),
        override_stale_check,
    )
    .map_err(|refusal| (kind, refusal))
}

/// The behind-base gate, live: fetch the base and the head, then refuse a
/// head behind its base. `aida pr ship` (both preflight passes) and the
/// drain's merge phase call it.
// trace:TASK-1606 | ai:codex
// trace:STORY-1690 | ai:claude
pub(crate) fn ship_behind_base_gate(
    project_root: &std::path::Path,
    ship_branch: &str,
    pr_number: u64,
    override_stale_check: bool,
) -> std::result::Result<(), (pr_ship::ShipRefusal, String)> {
    behind_base_gate_on(
        project_root,
        ship_branch,
        pr_number,
        &pr_ship_target_branch(pr_number),
        override_stale_check,
    )
}

/// [`ship_behind_base_gate`] against a known base branch.
// trace:STORY-1690 | ai:claude
pub(crate) fn behind_base_gate_on(
    project_root: &std::path::Path,
    ship_branch: &str,
    pr_number: u64,
    base_branch: &str,
    override_stale_check: bool,
) -> std::result::Result<(), (pr_ship::ShipRefusal, String)> {
    let staleness = fetch_ship_refs(project_root, base_branch, ship_branch).and_then(|()| {
        crate::pr_stale_check(project_root, ship_branch, &format!("origin/{base_branch}"))
    });
    behind_base_refusal(pr_number, base_branch, staleness, override_stale_check)
}

// trace:TASK-1606 | ai:codex
#[allow(clippy::too_many_arguments)]
fn ship_preflight_gates(
    project_root: &std::path::Path,
    main_worktree: &std::path::Path,
    hold_root: &std::path::Path,
    change: &crate::forge::ChangeRef,
    override_stale_check: bool,
    override_stale_approval: bool,
    delete_branch: bool,
    hold_authority: bool,
) -> Result<()> {
    use pr_ship::ShipRefusal;
    let forge = crate::forge::forge_for(project_root);
    let status = forge.change_status(change)?;
    if !status.mergeable {
        return Err(ShipRefusal::NotMergeable.error(format!(
            "PR-{} is not mergeable — rebase the branch and try again.",
            change.id
        )));
    }
    let ids = ship_gate_known_ids(project_root, &change.branch, change.title.as_deref());
    // BUG-1918: authors resolved from what the forge says the PR ships, not
    // only the local range (empty when shipping from main).
    let (candidates, policy) = merge_approval_inputs(
        &[project_root, main_worktree],
        main_worktree,
        change.id,
        &ids,
        Some(&change.branch),
        Some(&status.head_sha),
    );
    pr_ship_approval_gate(
        main_worktree,
        change.id,
        &candidates,
        Some(&status.head_sha),
        &policy,
        override_stale_approval,
        delete_branch,
        // TASK-1960: prove only. The gate that pins the merge records,
        // prints and logs the carry-over, once per ship.
        // trace:TASK-1960 | ai:claude
        Some(&crate::review_carryover::CarryOverSite::quiet(
            main_worktree,
            Some(change.base.as_str()),
        )),
    )?;
    // TASK-1968: the corpus gate honours the same clean-merge carry-over.
    // trace:TASK-1968 | ai:claude
    crate::merge_hold::enforce_corpus_gate_before_merge(
        project_root,
        change.id,
        &ids,
        Some(change.base.as_str()),
        || Some(status.head_sha.clone()),
    )
    .map_err(|e| ShipRefusal::Review.error(e))?;
    let gate = crate::review_marker::merge_gate(main_worktree, change.id, || {
        Some(status.head_sha.clone())
    });
    if let crate::review_marker::MergeGate::UnderReview(marker) = gate {
        return Err(ShipRefusal::Review.error(crate::review_marker::refusal_message(&marker)));
    }
    let marker = crate::merge_hold::read_hold(hold_root, change.id);
    let label_only =
        marker.is_none() && crate::merge_hold::label_present(hold_root, change.id).unwrap_or(false);
    // STORY-1698: a human at a terminal, or an advisor covered by an active
    // fire-fighting mandate. trace:STORY-1698 | ai:claude
    if let Some(refusal) = pr_ship::ship_hold_release_refusal_for(
        marker.is_some(),
        label_only,
        hold_authority,
        change.id,
    ) {
        return Err(ShipRefusal::MergeHold.error(refusal));
    }
    ship_behind_base_gate(
        project_root,
        &change.branch,
        change.id,
        override_stale_check,
    )
    .map_err(|(kind, refusal)| kind.error(refusal))
}

/// TASK-1602: whether this caller is a drive implementer/reviewer seat.
// trace:TASK-1602 | ai:codex
pub(crate) fn ship_caller_is_drive_seat() -> bool {
    std::env::var("AIDA_HEADLESS").is_ok_and(|v| v == "1")
        || std::env::var(orchestrator::AUTO_COMPLETE_ENV).is_ok_and(|v| !v.trim().is_empty())
}

/// TASK-1602: the in-drive ownership gate `aida pr ship` applies before it
/// merges PR `pr`: `Some` when a live drive owns the PR or the caller is a
/// drive seat. `aida pr train` applies it to every member.
// trace:TASK-1602 | ai:codex
// trace:STORY-1690 | ai:claude
pub(crate) fn probe_ship_drive_block(
    main_worktree: &std::path::Path,
    pr: u64,
) -> Option<pr_ship::ShipDriveBlock> {
    let allow_in_drive = std::env::var("AIDA_PR_SHIP_ALLOW_IN_DRIVE").is_ok_and(|v| v == "1");
    let scope = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(main_worktree, &cwd))
        .map(|lease| lease.scope);
    pr_ship::probe_ship_drive_guard(
        main_worktree,
        pr,
        scope.as_deref(),
        ship_caller_is_drive_seat(),
        allow_in_drive,
    )
}

/// BUG-727: a human at an interactive terminal is the explicit merge the
/// supervised-merge gate waits for.
// trace:BUG-727 | ai:claude
pub(crate) fn ship_interactive_tty() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdin())
        && std::io::IsTerminal::is_terminal(&std::io::stdout())
}

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
    override_stale_check: bool,
    override_stale_approval: bool,
    wait: Option<u64>,
    no_complete: bool,
) -> Result<()> {
    pr_ship_with_method(
        n,
        no_pull,
        no_cleanup,
        dry_run,
        force_delete_branch,
        complexity,
        effort,
        no_trailer_check,
        override_stale_check,
        override_stale_approval,
        wait,
        no_complete,
        crate::forge::MergeMethod::Squash,
        false,
    )
}

/// The refusal a post-release exit returns: when this ship's mandate
/// released a hold (a mandate cover and a hold marker), the hold is re-armed
/// and what `rearm` reports is appended; any other exit returns `refusal`
/// unchanged and re-arms nothing.
// trace:TASK-1955 | ai:claude (TASK-2088)
pub(crate) fn refusal_with_mandate_rearm(
    refusal: String,
    mandate_released: bool,
    held: bool,
    rearm: impl FnOnce() -> String,
) -> String {
    if mandate_released && held {
        format!("{refusal}{}", rearm())
    } else {
        refusal
    }
}

// trace:STORY-1663 | ai:antigravity
#[allow(clippy::too_many_arguments)]
pub(crate) fn pr_ship_with_method(
    n: Option<u64>,
    no_pull: bool,
    no_cleanup: bool,
    dry_run: bool,
    force_delete_branch: bool,
    complexity: Option<complexity_calibration::ComplexityLevel>,
    effort: Option<effort_calibration::EffortBucket>,
    no_trailer_check: bool,
    override_stale_check: bool,
    override_stale_approval: bool,
    wait: Option<u64>,
    no_complete: bool,
    merge_method: crate::forge::MergeMethod,
    final_slice: bool,
) -> Result<()> {
    pr_ship_with_method_and_dispatch_ctx(
        n,
        no_pull,
        no_cleanup,
        dry_run,
        force_delete_branch,
        complexity,
        effort,
        no_trailer_check,
        override_stale_check,
        override_stale_approval,
        wait,
        no_complete,
        merge_method,
        final_slice,
        None,
    )
}

// trace:STORY-1917 trace:TASK-2338 trace:TASK-2339 | ai:agy
#[allow(clippy::too_many_arguments)]
pub(crate) fn pr_ship_with_method_and_dispatch_ctx(
    n: Option<u64>,
    no_pull: bool,
    no_cleanup: bool,
    dry_run: bool,
    force_delete_branch: bool,
    complexity: Option<complexity_calibration::ComplexityLevel>,
    effort: Option<effort_calibration::EffortBucket>,
    no_trailer_check: bool,
    override_stale_check: bool,
    override_stale_approval: bool,
    wait: Option<u64>,
    no_complete: bool,
    merge_method: crate::forge::MergeMethod,
    final_slice: bool,
    dispatch_ctx: Option<&crate::gate::dispatch_auth::DispatchExecutionContext>,
) -> Result<()> {
    use pr_ship::{
        branch_pr_resolution_from_lookup, format_activity_event, format_dry_run_plan_with_method,
        recovery_hint, BranchPrResolution, PrShipOptions, ShipStep, StepOutcome,
    };
    if let Some(ctx) = dispatch_ctx {
        ctx.reverify_now(true)?;
    }
    // The one merge path behind `aida pr ship` and `aida ship`: a standalone
    // implementer stops at an open PR. trace:STORY-1662 | ai:claude
    crate::standalone_implementer::refuse_under_marker("pr ship")?;

    if final_slice && merge_method != crate::forge::MergeMethod::Rebase {
        anyhow::bail!(
            "`--final-slice` (`--complete-spec`) requires `--rebase` or `--merge-method rebase`"
        );
    }

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
    let forge_kind = crate::forge::resolve_forge_kind(&project_root);
    let main_worktree = main_worktree_root_from(&project_root);
    let hold_root = pr_ship_hold_root(&project_root);

    let branch = current_git_branch(&project_root)?;
    if branch.is_empty() {
        anyhow::bail!("could not detect current branch (detached HEAD?)");
    }

    // BUG-2164 / TASK-1795 / TASK-1803: On the default branch (or when the
    // default branch cannot be determined) with no explicit PR number, refuse
    // before any push, trailer guard, or PR creation attempt.
    // trace:BUG-2164 trace:TASK-1795 trace:TASK-1803 | ai:antigravity
    if opts.pr_number.is_none() {
        match classify_ship_default_branch(&project_root, &branch) {
            ShipDefaultBranchDisposition::FeatureBranch => {}
            ShipDefaultBranchDisposition::OnDefaultBranch => {
                let approved = find_open_approved_prs(&project_root);
                anyhow::bail!("{}", pr_ship::format_default_branch_ship_refusal(&approved));
            }
            ShipDefaultBranchDisposition::UnknownDefaultBranch => {
                let approved = find_open_approved_prs(&project_root);
                anyhow::bail!(
                    "{}",
                    pr_ship::format_unknown_default_branch_ship_refusal(&approved)
                );
            }
        }
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
                        // TASK-1442 (containment for BUG-1510): a leased branch
                        // must carry a commit trailered for the spec it's leased
                        // for before a PR opens under that spec's identity.
                        if let Some(expected) = list_leases(&main_worktree)
                            .into_iter()
                            .find(|l| l.branch == branch)
                            .map(|l| l.scope)
                        {
                            run_pr_open_spec_guard(&project_root, &branch, &expected);
                        }
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
        let plan = format_dry_run_plan_with_method(&opts, &steps, forge_kind, merge_method);
        // trace:BUG-1957 trace:STORY-1663 | ai:antigravity
        print!(
            "{}",
            pr_ship_dry_run_output_for_method(
                plan,
                &project_root,
                pr_number,
                &ship_branch,
                no_complete,
                merge_method,
                final_slice,
            )
        );
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

    // Reprobe the PR binding and caller on every poll; --wait never grants
    // authority to a drive seat or bypasses a merge hold.
    // trace:TASK-1602 | ai:codex
    let drive_seat = ship_caller_is_drive_seat();
    let direct_human = crate::seat_authority::require_direct_human().is_ok() && !drive_seat;
    // trace:TASK-1606 | ai:codex
    // One flag and deadline cover ownership plus CI registration/settlement.
    let wait_started = std::time::Instant::now();
    let result = pr_ship::wait_for_drive_release(
        wait.map(std::time::Duration::from_secs),
        std::time::Duration::from_secs(2),
        || probe_ship_drive_block(&main_worktree, pr_number),
        |block, remaining| {
            eprintln!(
                "  PR-{pr_number} waiting for drive release ({remaining:.0}s remaining): {}",
                block.detail
            );
        },
    );
    if let Err(block) = result {
        let reason_text = match block.reason {
            pr_ship::ShipMergeBlockReason::DriveSeat => {
                "caller is a drive implementer/reviewer seat"
            }
            pr_ship::ShipMergeBlockReason::DriveOwnedPr => "target PR is owned by the live drive",
        };
        if direct_human {
            eprintln!("  {}", block.detail);
            eprintln!("  The drive's independent reviewer gates the merge; on completion it merges if authorized or escalates the merge decision to you. No completion-time estimate is available.");
            eprintln!(
                "  Retry with `aida pr ship {pr_number} --wait 300` (bounded wait; no prompt)."
            );
        }
        log_ship_activity(
            &main_worktree,
            Some(pr_number),
            &pr_ship::ShipStep::Merge { delete_branch },
            &pr_ship::StepOutcome::Skipped(block.detail.clone()),
        );
        let refusal = if block.timed_out {
            pr_ship::ShipRefusal::CiTimeout
        } else {
            pr_ship::ShipRefusal::Review
        };
        return Err(refusal.error(format!(
            "PR-{pr_number} left OPEN — `aida pr ship` refused: {reason_text}. {} \
             Deliberate in-drive direct-publish: AIDA_PR_SHIP_ALLOW_IN_DRIVE=1 (never bypasses a merge-hold).",
            if block.timed_out { "Drive ownership wait timed out." } else { "The independent reviewer gates the merge." },
        )));
    }

    // Ownership may have been released because the drive merged the PR.
    // Refresh forge metadata before selecting the CI/merge or sync-only path.
    // trace:TASK-1602 | ai:codex
    if wait.is_some() && !already_merged {
        let mut probe_sink = crate::network_retry::NoopSink;
        already_merged =
            pr_is_merged_with_sink(&project_root, pr_number as u32, &mut probe_sink) == Some(true);
    }
    if let Some(ctx) = dispatch_ctx {
        ctx.reverify_now(true)?;
    }

    // ---- STORY-1698: the fire-fighting mandate. A human at a terminal
    // needs none; any other caller may release a hold only when an active
    // mandate covers THIS session (advisor seat of the minting human),
    // these specs, and an attested independent approval at the head. Read
    // fresh here, again under the merge lease, and once more immediately
    // before the merge call. trace:STORY-1698 | ai:claude
    let floor_authority = crate::has_integrity_floor_authority();
    let mandate_ids = || {
        let mut ids: Vec<String> = ship_gate_spec_records(&project_root)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        ids.extend(pr_ship::extract_spec_ids_from_text(&ship_branch));
        if let Some(title) = target_change.as_ref().and_then(|c| c.title.as_deref()) {
            ids.extend(pr_ship::extract_spec_ids_from_text(title));
        }
        ids
    };
    let mandate_cover_now = || {
        if pr_number == 0 || already_merged || floor_authority {
            return Err(crate::mandate::MandateRefusal::NoMandate);
        }
        mandate_ship_cover(
            &project_root,
            &main_worktree,
            &hold_root,
            pr_number,
            &mandate_ids(),
            Some(&ship_branch),
            || pr_head_sha_for_merge_gate(&project_root, pr_number, &ship_branch),
            override_stale_approval,
            crate::mandate::ship_decision,
            true,
        )
    };
    // BUG-2184: the mandate scan has its own cost and must not eat the --wait
    // budget meant for CI: its time is taken back out of the CI deadline.
    // trace:BUG-2184 | ai:claude
    let scan_started = std::time::Instant::now();
    let mut mandate_cover = mandate_cover_now();
    let scan_spent = scan_started.elapsed();
    let mandate_refusal_note =
        |cover: &std::result::Result<MandateCover, crate::mandate::MandateRefusal>| match cover {
            Err(r) if *r != crate::mandate::MandateRefusal::NoMandate => {
                let message = r.message(pr_number);
                crate::events::record_gate_held(
                    &hold_root,
                    crate::events::GATE_MANDATE_SHIP,
                    None,
                    Some(pr_number),
                    &message,
                );
                // trace:STORY-1700 | ai:claude
                crate::mandate_drive::ship_refused(&main_worktree, pr_number, &message);
                format!(" {message}.")
            }
            _ => String::new(),
        };

    // ---- BUG-1532: ANY merge-hold marker binds `aida pr ship`, with or
    // without a live drive and whatever its reason text names. Checked HERE —
    // before the CI watch (which would otherwise discount the hold's own red
    // gate) and before the release step. A supervision/decision hold still
    // releases for a human at a terminal (BUG-1167); a refusal-shaped hold
    // (rework/recusal/malformed/untyped) never releases through a merge, and
    // no hold releases without a human (BUG-1566). A label-only hold is
    // caught at the release step below, where the forge label is read.
    // AIDA_PR_SHIP_ALLOW_IN_DRIVE deliberately does not reach this gate.
    // trace:BUG-1532 | ai:claude
    if pr_number != 0 {
        if let Some(refusal) = pr_ship::ship_hold_gate(
            crate::merge_hold::read_hold_record(&main_worktree, pr_number).as_ref(),
            floor_authority || mandate_cover.is_ok(),
            pr_number,
            |record| {
                // BUG-1532 criterion 4: only for a refusal hold with a human
                // present — read the PR's live head, then the verdict at it.
                let head = pr_head_sha_for_merge_gate(&project_root, pr_number, &ship_branch);
                ship_refusal_release(
                    &[main_worktree.as_path(), project_root.as_path()],
                    record,
                    head.as_deref(),
                )
            },
        ) {
            let refusal = format!("{refusal}{}", mandate_refusal_note(&mandate_cover));
            log_ship_activity(
                &main_worktree,
                Some(pr_number),
                &pr_ship::ShipStep::Merge { delete_branch },
                &pr_ship::StepOutcome::Skipped(refusal.clone()),
            );
            // STORY-1436: the ship hold-release floor HELD. trace:STORY-1436 | ai:claude
            crate::events::record_gate_held(
                &hold_root,
                crate::events::GATE_SHIP_HOLD_RELEASE,
                None,
                Some(pr_number),
                &refusal,
            );
            return Err(pr_ship::ShipRefusal::MergeHold.error(refusal));
        }
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
        return Err(
            pr_ship::ShipRefusal::Review.error("PR left open — awaiting independent review")
        );
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
    let interactive_tty = ship_interactive_tty();
    let supervised = if already_merged {
        Vec::new()
    } else {
        // STORY-1698: an advisor covered by an active mandate is the explicit
        // advisor merge this gate waits for (operator-only specs never are:
        // the mandate refuses them). trace:STORY-1698 | ai:claude
        supervised_specs_for_ship(&project_root, interactive_tty || mandate_cover.is_ok())
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
        return Err(
            pr_ship::ShipRefusal::Review.error("PR left open — awaiting independent review")
        );
    }

    let gate_change = crate::forge::ChangeRef {
        id: pr_number,
        url: String::new(),
        branch: ship_branch.clone(),
        base: retarget_base.clone(),
        title: target_change.as_ref().and_then(|c| c.title.clone()),
    };
    let preflight_head_sha = if !already_merged {
        ship_preflight_gates(
            &project_root,
            &main_worktree,
            &hold_root,
            &gate_change,
            override_stale_check,
            override_stale_approval,
            delete_branch,
            floor_authority || mandate_cover.is_ok(),
        )?;
        pr_head_sha_for_merge_gate(&project_root, pr_number, &ship_branch)
    } else {
        None
    };

    // BUG-574: if the PR is already merged (re-run on an already-shipped spec,
    // or a sibling beat us to the merge) there is nothing to watch or merge.
    // Skip straight to the idempotent pull + cleanup so the re-run exits 0
    // instead of erroring out of a no-op `gh pr merge`. trace:BUG-574 | ai:claude
    let mut label_only_hold = false;
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
        // Route registration and check reads through the Forge trait.
        // trace:STORY-516 trace:TASK-1331 | ai:codex
        let watch_change = crate::forge::ChangeRef {
            id: pr_number,
            url: String::new(),
            branch: ship_branch.clone(),
            base: String::new(),
            title: None,
        };
        let forge = crate::forge::forge_for(&project_root);
        // trace:TASK-1606 | ai:codex
        let mut red_detail: Option<String> = None;
        let hold_present = crate::merge_hold::read_hold(&hold_root, pr_number).is_some();
        label_only_hold = !hold_present
            && crate::merge_hold::label_present(&hold_root, pr_number).unwrap_or(false);
        let ci_result = crate::ci_gate::wait_for_ship_ci(
            &project_root,
            forge.as_ref(),
            &watch_change,
            hold_present,
            label_only_hold,
            wait.map(|secs| ship_ci_budget(secs, wait_started.elapsed(), scan_spent)),
            std::time::Duration::from_secs(15),
        );
        if let Err(error) = &ci_result {
            red_detail = Some(format!("{error:#}"));
        }
        let ci_failed = matches!(ci_result, Ok(crate::forge::CiState::Failed) | Err(_));
        // STORY-1480: the ship's CI wait reached a terminal verdict — emit
        // the same CiTerminal the drain's CI phase emits (empty run_uuid;
        // specs credited from the branch name), covering interactive ships.
        // Emitted after the BUG-1180 refinement so a hold/matrix pseudo-red
        // records as the green it really was. trace:STORY-1480 | ai:claude
        crate::events::emit_interactive_lifecycle(
            &main_worktree,
            &pr_ship::extract_spec_ids_from_text(&ship_branch),
            &crate::events::EventKind::CiTerminal { green: !ci_failed },
        );
        if ci_failed {
            let detail = match &ci_result {
                Err(e) => format!("{e:#}"),
                _ => red_detail
                    .clone()
                    .unwrap_or_else(|| format!("CI did not pass for PR-{pr_number}")),
            };
            log_ship_activity(
                &main_worktree,
                Some(pr_number),
                &ShipStep::WatchCi,
                &StepOutcome::Failed(detail),
            );
            let hint = recovery_hint(&ShipStep::WatchCi, Some(pr_number), forge_kind);
            eprintln!(
                "{} CI failed for PR-{}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                pr_number
            );
            eprintln!("  {}", hint);
            return Err(match ci_result {
                Err(e) => e,
                _ => pr_ship::ShipRefusal::CiRed.error(
                    red_detail.unwrap_or_else(|| format!("CI did not pass for PR-{pr_number}")),
                ),
            });
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
    if let Some(ctx) = dispatch_ctx {
        ctx.reverify_now(true)?;
    }

    // ---- Step 3: merge. Use retry-wrapper so a transient gh
    // network blip doesn't abort. ----
    // BUG-574: skipped entirely when the PR is already merged — the activity
    // log for the merge step was already recorded as Skipped above, and the
    // idempotent pull + cleanup below still run. trace:BUG-574 | ai:claude
    let mut merged_this_run = false;
    // STORY-1171 / TASK-1243: hold the branch merge-lease across check→merge→pull
    // (ADR-38) so no two same-machine mergers race to this branch. Keyed on the
    // PR's base branch, held on the MAIN clone (all mergers resolve here), and
    // released when this handler returns. Bounded-wait-then-refuse on contention.
    // trace:STORY-1171 | ai:claude
    let _merge_lease = acquire_merge_lease(&main_worktree, pr_number)?;
    if let Some(ctx) = dispatch_ctx {
        ctx.reverify_now(true)?;
    }
    // STORY-1698: re-read the mandate under the merge lease. A mandate that
    // covered the early gate but lapsed (or whose approval moved) since
    // refuses here, before any hold is touched. trace:STORY-1698 | ai:claude
    if mandate_cover.is_ok() {
        mandate_cover = mandate_cover_now();
        if mandate_cover.is_err() {
            let refusal = format!(
                "PR-{pr_number} left OPEN — the merge-hold stays.{}",
                mandate_refusal_note(&mandate_cover)
            );
            log_ship_activity(
                &main_worktree,
                Some(pr_number),
                &pr_ship::ShipStep::Merge { delete_branch },
                &pr_ship::StepOutcome::Skipped(refusal.clone()),
            );
            return Err(pr_ship::ShipRefusal::MergeHold.error(refusal));
        }
    }
    if !already_merged {
        ship_preflight_gates(
            &project_root,
            &main_worktree,
            &hold_root,
            &gate_change,
            override_stale_check,
            override_stale_approval,
            delete_branch,
            floor_authority || mandate_cover.is_ok(),
        )?;
    }
    // STORY-1405: refuse while a reviewer is mid-way through THIS head. Asked
    // under the merge-lease, immediately before the merge, so a review that
    // started after CI settled is still seen. One local stat when no marker
    // exists; a forge head lookup only when a live one does.
    // trace:STORY-1405 | ai:claude
    if !already_merged {
        let gate = crate::review_marker::merge_gate(&main_worktree, pr_number, || {
            fetch_change_info_via_resolved_forge(
                &project_root,
                pr_number,
                crate::forge::resolve_open_change_forge_kind(&project_root),
            )
            .ok()
            .map(|info| info.head_oid)
        });
        if let crate::review_marker::MergeGate::UnderReview(m) = &gate {
            let refusal = crate::review_marker::refusal_message(m);
            // STORY-1436: the review-in-progress gate HELD.
            // trace:STORY-1436 | ai:claude
            crate::events::record_gate_held(
                &hold_root,
                crate::events::GATE_REVIEW_IN_PROGRESS,
                None,
                Some(pr_number),
                &refusal,
            );
            return Err(pr_ship::ShipRefusal::Review.error(refusal));
        }
        if let Some(note) = crate::review_marker::proceed_note(&gate) {
            eprintln!(
                "  {} {note}",
                crate::glyph(crate::glyphs::Glyph::Info).cyan()
            );
        }
    }
    let mut merged_head_sha: Option<String> = None;
    if !already_merged {
        // ---- TASK-1448: approval-covers-head gate. Checked under the merge
        // lease, immediately before the merge (and before any merge-hold is
        // released), so the head it compares is the one about to land. ----
        // trace:TASK-1448 | ai:claude
        let mut gate_spec_ids: Vec<String> = ship_gate_spec_records(&project_root)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        gate_spec_ids.extend(pr_ship::extract_spec_ids_from_text(&ship_branch));
        if let Some(title) = target_change.as_ref().and_then(|c| c.title.as_deref()) {
            gate_spec_ids.extend(pr_ship::extract_spec_ids_from_text(title));
        }
        let head_sha = pr_head_sha_for_merge_gate(&project_root, pr_number, &ship_branch);
        merged_head_sha = head_sha.clone();
        // STORY-1663: if the PR head moved after preflight/CI, refuse the stale head
        // before touching holds or merging.
        // trace:STORY-1663 | ai:antigravity
        if let Some(refusal) = pr_ship::stale_head_after_ci_refusal(
            pr_number,
            preflight_head_sha.as_deref(),
            head_sha.as_deref(),
        ) {
            log_ship_activity(
                &main_worktree,
                Some(pr_number),
                &pr_ship::ShipStep::Merge { delete_branch },
                &pr_ship::StepOutcome::Failed(refusal.clone()),
            );
            crate::events::record_gate_held(
                &hold_root,
                crate::events::GATE_STALE_APPROVAL,
                None,
                Some(pr_number),
                &refusal,
            );
            return Err(pr_ship::ShipRefusal::Review.error(refusal));
        }
        // BUG-1918: what the PR ships comes from the forge too (head ref and
        // commit trailers), so shipping from main still finds its authors.
        let (candidates, policy) = merge_approval_inputs(
            &[project_root.as_path(), main_worktree.as_path()],
            &main_worktree,
            pr_number,
            &gate_spec_ids,
            Some(&ship_branch),
            head_sha.as_deref(),
        );
        // TASK-1960: the site remembers an applied carry-over, so the PR's
        // base is re-checked against the proven base right before the merge.
        // trace:TASK-1960 | ai:claude
        let carry_site = crate::review_carryover::CarryOverSite::new(
            &main_worktree,
            Some(retarget_base.as_str()),
        );
        // TASK-1458: gate + merge pin + durable override audit, in one
        // function so the wiring test drives exactly what ship runs.
        // trace:TASK-1458 | ai:claude
        let match_head = pr_ship_approval_gate(
            &main_worktree,
            pr_number,
            &candidates,
            head_sha.as_deref(),
            &policy,
            override_stale_approval,
            delete_branch,
            Some(&carry_site),
        )?;
        // STORY-1663: optional slice-stack verifier hook before rebase merge.
        // trace:STORY-1663 | ai:antigravity
        if merge_method == crate::forge::MergeMethod::Rebase {
            run_optional_slice_stack_verifier(
                &project_root,
                &main_worktree,
                pr_number,
                &format!("origin/{retarget_base}"),
                head_sha.as_deref().unwrap_or(&ship_branch),
            )?;
        }
        if branch_in_sibling {
            eprintln!(
                "  step 3: branch {} is checked out in a sibling worktree — \
                 skipping `--delete-branch`; `aida session end` will clean it up",
                ship_branch
            );
        }
        let method_label = match merge_method {
            crate::forge::MergeMethod::Squash => "squash",
            crate::forge::MergeMethod::Rebase => "rebase",
            crate::forge::MergeMethod::Merge => "merge",
        };
        eprintln!(
            "  step 3: merging PR-{} ({}{})",
            pr_number,
            method_label,
            if delete_branch { ", delete-branch" } else { "" }
        );
        // BUG-1919 / STORY-1663: say which ids the merge will complete and where
        // they came from (title / branch / none) BEFORE the merge, so a
        // branch-name fallback or non-final rebase slice never completes a spec
        // silently. Rebase merges produce no squash subject.
        // trace:BUG-1919 trace:STORY-1663 | ai:antigravity
        let ship_meta = fetch_pr_ship_metadata_via_gh(&project_root, pr_number)?;
        let (explicit_squash_subject, completion_trailer) = derive_pr_ship_subject_for_method_from(
            &project_root,
            &ship_meta,
            &ship_branch,
            no_complete,
            merge_method,
            final_slice,
        )?;
        // TASK-1765: the forge's squash body is built from the commits, not
        // the PR body, so a non-completing declaration would not reach main
        // and the pull's review-story covers path would complete the covered
        // specs. trace:TASK-1951 trace:TASK-1765 | ai:claude
        let declares_non_completing =
            no_complete || pr_ship::pr_body_declares_non_completing(&ship_meta.body);
        eprintln!(
            "  step 3: {}",
            pr_ship::format_trailer_provenance(&completion_trailer)
        );
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
        // TASK-1330: identity hygiene — when the project allowlists emails
        // and the default squash body would carry a non-allowlisted one
        // (an explicit Co-authored-by trailer, or a commit author/committer
        // the forge would auto-derive a trailer from), replace the body with
        // the sanitized commit messages. Fail-closed: an unreadable config or
        // uninspectable branch refuses the merge rather than publishing an
        // unchecked body. trace:TASK-1330 | ai:claude
        let identity_allowlist = crate::identity_gate::read_allowed_emails(&project_root)?;
        let sanitized_squash_body = if merge_method == crate::forge::MergeMethod::Squash {
            let override_body = crate::identity_gate::squash_coauthor_body_override(
                &project_root,
                &retarget_base,
                head_sha.as_deref().unwrap_or(&ship_branch),
                &identity_allowlist,
            )?;
            override_body.map(|(body, violations)| {
                eprintln!(
                    "  step 3: identity gate — stripping {} non-allowlisted email reference(s) \
                     from the squash body:",
                    violations.len()
                );
                for v in &violations {
                    eprintln!(
                        "    {} {} {}",
                        &v.sha[..v.sha.len().min(10)],
                        v.role,
                        v.email
                    );
                }
                body
            })
        } else {
            None
        };
        let sanitized_squash_body = ship_squash_body(
            merge_method,
            declares_non_completing,
            sanitized_squash_body,
            || {
                crate::identity_gate::squash_commit_messages_body(
                    &project_root,
                    &retarget_base,
                    head_sha.as_deref().unwrap_or(&ship_branch),
                )
            },
        )?;
        let mut merge_opts = crate::forge::MergeOptions {
            method: merge_method,
            squash_subject: explicit_squash_subject.clone(),
            squash_body: sanitized_squash_body, // trace:TASK-1330 | ai:claude
            delete_branch,
            match_head, // trace:TASK-1458 | ai:claude
        };
        let change_ref = crate::forge::ChangeRef {
            id: pr_number,
            url: String::new(),
            branch: ship_branch.clone(),
            base: retarget_base.clone(),
            title: None,
        };
        // BUG-1167: `aida pr merge` / `pr ship` is an EXPLICIT human/advisor
        // merge — the deliberate act that "merge requires human/advisor review"
        // asks for. So it RELEASES any supervised merge-hold marker before
        // merging (the chokepoint would otherwise refuse it). The automatic and
        // concurrent paths — drain auto-merge, an integrate sweep, another
        // session's merge_change — never clear, so they stay refused. This is
        // the client-side release; the server-side required-check (ADR-37 layer
        // 2) still governs a raw `gh pr merge`.
        //
        // BUG-1566: releasing the hold here is the SAME human-only integrity
        // floor as `merge-hold clear` (has_integrity_floor_authority(),
        // STORY-1353). A human shipping at a terminal still gets BUG-1167's
        // one-step release; any other seat (headless drain, advisor/reviewer
        // env, dispatch) is refused BEFORE the hold is touched and pointed at
        // `aida merge-hold clear <PR>` run by a human. A reviewer's "clear on
        // my re-review" hold therefore moves to a human once approved.
        // trace:BUG-1167 trace:BUG-1566 | ai:claude
        if let Some(ctx) = dispatch_ctx {
            ctx.reverify_now(true)?;
        }
        let marker_reason = crate::merge_hold::read_hold(&hold_root, pr_number);
        // BUG-1499: a LABEL-ONLY hold (no marker) is never released here —
        // not even for a human — because a ship leaves no clearance record.
        // It goes through `aida merge-hold clear <PR>`, which records it.
        // trace:BUG-1499 | ai:claude
        // STORY-1698: the mandate cover was re-read under the merge lease.
        // trace:STORY-1698 | ai:claude
        let mandate_release = (!floor_authority)
            .then(|| mandate_cover.as_ref().ok().cloned())
            .flatten();
        if let Some(refusal) = pr_ship::ship_hold_release_refusal_for(
            marker_reason.is_some(),
            label_only_hold,
            floor_authority || mandate_release.is_some(),
            pr_number,
        ) {
            log_ship_activity(
                &main_worktree,
                Some(pr_number),
                &pr_ship::ShipStep::Merge { delete_branch },
                &pr_ship::StepOutcome::Skipped(refusal.clone()),
            );
            // STORY-1436: the ship hold-release floor HELD. trace:STORY-1436 | ai:claude
            crate::events::record_gate_held(
                &hold_root,
                crate::events::GATE_SHIP_HOLD_RELEASE,
                None,
                Some(pr_number),
                &refusal,
            );
            // trace:TASK-1915 | ai:claude
            ledger_mandate_ship_failure(
                &main_worktree,
                mandate_release.as_ref(),
                pr_number,
                &refusal,
            );
            return Err(pr_ship::ShipRefusal::MergeHold.error(refusal));
        }
        // BUG-1532 criterion 4, re-checked HERE under the merge lease against
        // the head about to land (`head_sha`, the TASK-1448 read): a refusal
        // hold is released only by a fresh APPROVED verdict at that head. The
        // early gate saw the same answer unless the head or the verdict moved
        // since, and then this refuses rather than release on a stale answer.
        // trace:BUG-1532 | ai:claude
        let hold_record = crate::merge_hold::read_hold_record(&hold_root, pr_number);
        let released_by_verdict = match hold_record.as_ref().filter(|r| r.is_refusal()) {
            None => None,
            Some(record) => match ship_refusal_release(
                &[hold_root.as_path(), project_root.as_path()],
                record,
                head_sha.as_deref(),
            ) {
                crate::merge_hold::RefusalRelease::Released(verdict) => Some(verdict),
                crate::merge_hold::RefusalRelease::Held(why) => {
                    let refusal = format!(
                        "PR-{pr_number} is under a reviewer-refusal merge-hold and {why}; the hold stays."
                    );
                    log_ship_activity(
                        &main_worktree,
                        Some(pr_number),
                        &pr_ship::ShipStep::Merge { delete_branch },
                        &pr_ship::StepOutcome::Skipped(refusal.clone()),
                    );
                    // STORY-1436. trace:STORY-1436 | ai:claude
                    crate::events::record_gate_held(
                        &hold_root,
                        crate::events::GATE_SHIP_HOLD_RELEASE,
                        None,
                        Some(pr_number),
                        &refusal,
                    );
                    // trace:TASK-1915 | ai:claude
                    ledger_mandate_ship_failure(
                        &main_worktree,
                        mandate_release.as_ref(),
                        pr_number,
                        &refusal,
                    );
                    return Err(pr_ship::ShipRefusal::MergeHold.error(refusal));
                }
            },
        };
        // BUG-1532: a hold released by a verdict must merge PINNED to the head
        // that verdict covers — the TASK-1448 pin is None when the verdict's
        // key is not among its candidates, and an unpinned merge after the
        // hold drops could land a commit nobody approved. Decided BEFORE the
        // hold is touched. trace:BUG-1532 | ai:claude
        match pr_ship::refusal_release_pin(
            released_by_verdict.is_some(),
            merge_opts.match_head.take(),
            head_sha.as_deref(),
            pr_number,
        ) {
            Ok(pin) => merge_opts.match_head = pin,
            Err(refusal) => {
                log_ship_activity(
                    &main_worktree,
                    Some(pr_number),
                    &pr_ship::ShipStep::Merge { delete_branch },
                    &pr_ship::StepOutcome::Skipped(refusal.clone()),
                );
                // STORY-1436. trace:STORY-1436 | ai:claude
                crate::events::record_gate_held(
                    &hold_root,
                    crate::events::GATE_SHIP_HOLD_RELEASE,
                    None,
                    Some(pr_number),
                    &refusal,
                );
                // trace:TASK-1915 | ai:claude
                ledger_mandate_ship_failure(
                    &main_worktree,
                    mandate_release.as_ref(),
                    pr_number,
                    &refusal,
                );
                return Err(pr_ship::ShipRefusal::MergeHold.error(refusal));
            }
        }
        // STORY-1663: rebase merges must always be pinned to a head SHA.
        // trace:STORY-1663 | ai:antigravity
        if let Err(refusal) = pr_ship::require_rebase_head_pin(
            pr_number,
            merge_opts.method,
            merge_opts.match_head.as_deref(),
        ) {
            log_ship_activity(
                &main_worktree,
                Some(pr_number),
                &pr_ship::ShipStep::Merge { delete_branch },
                &pr_ship::StepOutcome::Failed(refusal.clone()),
            );
            // trace:TASK-1915 | ai:claude
            ledger_mandate_ship_failure(
                &main_worktree,
                mandate_release.as_ref(),
                pr_number,
                &refusal,
            );
            return Err(pr_ship::ShipRefusal::Review.error(refusal));
        }
        if let Some(reason) = marker_reason.as_deref() {
            match &released_by_verdict {
                Some(verdict) => eprintln!(
                    "  {} releasing reviewer-refusal merge-hold on PR-{} — its condition is met: \
                     a fresh APPROVED verdict for {} at {} ({})",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    pr_number,
                    verdict.key,
                    verdict
                        .reviewed_sha
                        .as_deref()
                        .map(crate::review_verdict::short_sha)
                        .unwrap_or("?"),
                    verdict.path(),
                ),
                None => eprintln!(
                    "  {} releasing supervised merge-hold on PR-{} (explicit review-merge) — {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    pr_number,
                    reason,
                ),
            }
            // STORY-1698: a mandate-authorized release is recorded in the
            // hold ledger and on the PR as "released under mandate <id> by
            // advisor". BUG-2214: through the one shared release sequence the
            // train also uses. trace:STORY-1698 trace:BUG-2214 | ai:claude
            if let (Some(cover), Some(record)) = (&mandate_release, &hold_record) {
                release_hold_under_mandate(
                    &hold_root,
                    &change_ref,
                    cover,
                    record,
                    released_by_verdict.clone(),
                    head_sha.as_deref(),
                    reason,
                );
            } else {
                let _ = crate::merge_hold::clear_hold(&hold_root, pr_number);
                if let (Some(record), Some(verdict)) = (&hold_record, &released_by_verdict) {
                    let actor =
                        crate::merge_hold::PrincipalIdentity::human(crate::current_user_id(None));
                    if let Err(err) = crate::merge_hold::record_clearance_with_verdict(
                        &hold_root,
                        record,
                        &actor,
                        Some(verdict.clone()),
                    ) {
                        eprintln!(
                            "  {} could not record the hold release on PR-{pr_number}: {err}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                        );
                    }
                }
                // BUG-1423: this IS the coordination-seat action the missing
                // event taxonomy could not record — an explicit advisor/human
                // `aida pr ship` releasing a supervised merge-hold. Best-effort,
                // never affects the ship outcome. trace:BUG-1423 | ai:claude
                let mut hold_event = crate::events::Event::new(
                    None,
                    "",
                    crate::events::EventKind::MergeHoldChanged {
                        pr: pr_number as u32,
                        placed: false,
                        reason: Some(reason.to_string()),
                        // BUG-2214: who released, and whether the human
                        // integrity floor held. trace:BUG-2214 | ai:claude
                        principal: Some(
                            crate::merge_hold::PrincipalIdentity::human(crate::current_user_id(
                                None,
                            ))
                            .key(),
                        ),
                        human_attested: crate::has_integrity_floor_authority(),
                    },
                );
                hold_event.seat = crate::events::active_seat();
                crate::events::emit(&hold_root, &hold_event);
                if let Err(err) = crate::merge_hold::sync_label(&hold_root, pr_number, false) {
                    eprintln!(
                        "  {} {}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        ship_unlabel_failure(pr_number, &err),
                    );
                }
            }
            // BUG-1180: removing the label re-runs `merge-hold-gate`; the
            // squash-merge below would be refused by branch protection until it
            // reports green, so wait for it (bounded) before merging.
            // trace:BUG-1180 | ai:claude
            {
                match crate::ci_gate::wait_hold_gate_green(
                    crate::forge::forge_for(&hold_root).as_ref(),
                    &change_ref,
                    std::time::Duration::from_secs(180),
                    std::time::Duration::from_secs(10),
                ) {
                    Ok(true) => {}
                    // STORY-1698: a mandate release never stays released on
                    // a failure. One exit for both arms, so the ledger call
                    // is the mandate arm's real write, not a no-op kept for
                    // the wiring pin. trace:STORY-1698 | ai:claude
                    // trace:TASK-1955 | ai:claude (TASK-2086)
                    Ok(false) => {
                        let refusal = if mandate_release.is_some() {
                            format!(
                                "released the merge-hold on PR-{pr_number} under a mandate, but `{}` has not re-run green within 180s.{}",
                                crate::ci_gate::HOLD_GATE_CHECK,
                                rearm_after_mandate_failure(&hold_root, hold_record.as_ref(), pr_number)
                            )
                        } else {
                            format!(
                                "released the merge-hold on PR-{pr_number}, but `{}` has not re-run green within \
                                 180s — retry `aida pr ship {pr_number}` shortly (the hold stays released), or \
                                 inspect `gh pr checks {pr_number}`",
                                crate::ci_gate::HOLD_GATE_CHECK
                            )
                        };
                        // trace:TASK-1915 | ai:claude
                        ledger_mandate_ship_failure(
                            &main_worktree,
                            mandate_release.as_ref(),
                            pr_number,
                            &refusal,
                        );
                        anyhow::bail!(refusal)
                    }
                    Err(e) => eprintln!(
                        "  {} could not confirm `{}` re-ran green ({e:#}) — attempting the merge anyway",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        crate::ci_gate::HOLD_GATE_CHECK
                    ),
                }
            }
        }
        // STORY-1698: re-read the mandate immediately before the merge call.
        // If it lapsed after the hold was released (the hold-gate wait above
        // can take minutes), re-arm the hold and refuse.
        // trace:STORY-1698 trace:STORY-1917 trace:TASK-2344 | ai:claude
        let dispatch_ctx_err = dispatch_ctx.and_then(|ctx| ctx.reverify_now(true).err());
        if dispatch_ctx_err.is_some()
            || mandate_release
                .as_ref()
                .is_some_and(|cover| !mandate_still_active(&cover.id))
        {
            let cover_id = mandate_release
                .as_ref()
                .map(|c| c.id.as_str())
                .or_else(|| dispatch_ctx.map(|c| c.mandate_id()))
                .unwrap_or("unknown");
            let detail = match dispatch_ctx_err {
                Some(err) => format!("{cover_id} ({err:#})"),
                None => cover_id.to_string(),
            };
            let refusal = format!(
                "PR-{pr_number} left OPEN — mandate {detail} ended before the merge.{}",
                rearm_after_mandate_failure(
                    &hold_root,
                    hold_record.as_ref().filter(|_| mandate_release.is_some()),
                    pr_number,
                )
            );
            crate::events::record_gate_held(
                &hold_root,
                crate::events::GATE_MANDATE_SHIP,
                None,
                Some(pr_number),
                &refusal,
            );
            // trace:TASK-1915 | ai:claude
            ledger_mandate_ship_failure(
                &main_worktree,
                mandate_release.as_ref(),
                pr_number,
                &refusal,
            );
            return Err(pr_ship::ShipRefusal::MergeHold.error(refusal));
        }
        // TASK-1960: a carried-over approval is bound to the base it was
        // proven against; re-read the PR's base immediately before merging.
        // trace:TASK-1960 | ai:claude
        if let Some(proven_base) = carry_site.proven_base() {
            let base_now = crate::forge::forge_for(&project_root)
                .change_metadata(pr_number, &mut crate::network_retry::NoopSink)
                .ok()
                .map(|m| m.base_ref);
            if let Some(refusal) = crate::review_carryover::proven_base_refusal(
                pr_number,
                &proven_base,
                base_now.as_deref(),
            ) {
                log_ship_activity(
                    &main_worktree,
                    Some(pr_number),
                    &pr_ship::ShipStep::Merge { delete_branch },
                    &pr_ship::StepOutcome::Failed(refusal.clone()),
                );
                crate::events::record_gate_held(
                    &hold_root,
                    crate::events::GATE_STALE_APPROVAL,
                    None,
                    Some(pr_number),
                    &refusal,
                );
                // The hold a mandate released is re-armed and the refusal ledgered,
                // like every other post-release exit. trace:TASK-1915 | ai:claude
                // trace:TASK-1955 | ai:claude (TASK-2088)
                let refusal = refusal_with_mandate_rearm(
                    refusal,
                    mandate_release.is_some(),
                    marker_reason.is_some(),
                    || rearm_after_mandate_failure(&hold_root, hold_record.as_ref(), pr_number),
                );
                ledger_mandate_ship_failure(
                    &main_worktree,
                    mandate_release.as_ref(),
                    pr_number,
                    &refusal,
                );
                return Err(pr_ship::ShipRefusal::Review.error(refusal));
            }
        }
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
                let hint = recovery_hint(
                    &ShipStep::Merge { delete_branch },
                    Some(pr_number),
                    forge_kind,
                );
                eprintln!(
                    "{} merge failed: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    stderr_text
                );
                eprintln!("  {}", hint);
                // STORY-1698: re-arm a mandate-released hold on a failed merge.
                // trace:STORY-1698 | ai:claude
                if mandate_release.is_some() && marker_reason.is_some() {
                    let rearm =
                        rearm_after_mandate_failure(&hold_root, hold_record.as_ref(), pr_number);
                    // trace:TASK-1915 | ai:claude
                    ledger_mandate_ship_failure(
                        &main_worktree,
                        mandate_release.as_ref(),
                        pr_number,
                        &format!("`gh pr merge` failed: {stderr_text}.{rearm}"),
                    );
                    return Err(e.context(format!("`gh pr merge` failed.{rearm}")));
                }
                // A mandate ship with no hold to re-arm is still a failed
                // mandate-driven action. trace:TASK-1915 | ai:claude
                ledger_mandate_ship_failure(
                    &main_worktree,
                    mandate_release.as_ref(),
                    pr_number,
                    &format!("`gh pr merge` failed: {stderr_text}"),
                );
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
        // STORY-1700: a ship under a mandate is a mandate-driven action.
        // trace:STORY-1700 | ai:claude
        if let (true, Some(cover)) = (merged_this_run, &mandate_release) {
            crate::mandate_drive::emit_session_ship(
                &main_worktree,
                &cover.id,
                pr_number,
                crate::mandate_drive::Outcome::Done,
                None,
            );
        }
    }

    // STORY-439: ship-side calibration capture. Resolve every spec the PR
    // credits (explicit title trailer → branch, the same precedence the squash
    // subject repair already uses) and write a ship slot per spec — the
    // implementer's self-assessed complexity + the punt count
    // (`.aida/punts.jsonl` filtered by spec). One PR crediting N specs
    // populates N records. Best-effort: a `gh` blip here leaves the merge
    // landed and just skips the capture.
    // trace:STORY-439 | ai:claude
    // BUG-1423: the decisive gap — a merge performed by `aida pr ship`
    // (the advisor/product seat's explicit merge path) never appended a
    // `PrMerged` event, so the feed recorded only drain merges. Mirror the
    // drain merge phase's emit here (`DriveContext::merge_pr` in `lib.rs`)
    // so a feed consumer cannot tell whether a merge came from the
    // orchestrator or this command — same kind, same terminal semantics.
    // Best-effort: emit failures never fail the ship. trace:BUG-1423 | ai:claude
    let mut pr_merged_spec_ids: Vec<String> = Vec::new();
    let mut pr_title_body: Option<(String, String)> = None;
    if let Ok(pr_meta) = fetch_pr_ship_metadata_via_gh(&project_root, pr_number) {
        pr_title_body = Some((pr_meta.title.clone(), pr_meta.body.clone()));
        // Use the same store-qualified completion intent as the squash subject
        // — including BUG-1919's non-completing / closure-held suppression, so
        // a slice PR is not credited as a ship of its parent either.
        // trace:TASK-1600 | ai:codex
        // trace:BUG-1919 | ai:claude
        let spec_ids = Storage::new(main_worktree.join(".aida-store"))
            .load()
            .map(|store| {
                ship_completion_trailer_for_method(
                    &store,
                    merge_method,
                    final_slice,
                    &pr_meta.title,
                    &branch,
                    &pr_meta.body,
                    no_complete,
                )
                .ids
            })
            .unwrap_or_default();
        pr_merged_spec_ids = spec_ids.clone();
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
    // BUG-1423: emit only for a merge THIS invocation performed — matches
    // the drain's own emit-on-success semantics and keeps a re-run against
    // an already-merged PR (BUG-574) from re-appending a duplicate event.
    // A PR crediting no resolvable spec still emits once (spec: None) so
    // the merge is never silently dropped from the feed.
    // trace:BUG-1423 | ai:claude
    if merged_this_run {
        emit_ship_pr_merged(&main_worktree, pr_number as u32, &pr_merged_spec_ids);
    }
    // TASK-1621: the slice bookkeeping also runs on a re-run that finds the PR
    // already merged, so a crash between the merge and this step is repaired
    // by re-running `aida pr ship N --rebase`. On that path the head branch,
    // base and head sha come from the merged PR itself, never from the
    // current checkout. Completion and the marker are both idempotent (the
    // marker dedups per PR).
    // trace:TASK-1951 trace:TASK-1621 | ai:claude
    let mut slice_pull_skip = None;
    if rebase_slice_bookkeeping_due(merged_this_run, already_merged)
        && merge_method == crate::forge::MergeMethod::Rebase
    {
        let outcome = rebase_slice_bookkeeping(
            &RebaseSliceRun {
                main_worktree: &main_worktree,
                project_root: &project_root,
                pr_number,
                final_slice,
                already_merged,
                ship_branch: &ship_branch,
                retarget_base: &retarget_base,
                merged_head_sha: merged_head_sha.as_deref(),
            },
            &pr_merged_spec_ids,
            |merged_branch: &str| {
                // TASK-1621 (a): the merged PR's own branch, not the checkout's.
                let (title, body) = pr_title_body.clone().unwrap_or_default();
                Storage::new(main_worktree.join(".aida-store"))
                    .load()
                    .map(|store| {
                        ship_completion_trailer_for_method(
                            &store,
                            merge_method,
                            final_slice,
                            &title,
                            merged_branch,
                            &body,
                            no_complete,
                        )
                        .ids
                    })
                    .unwrap_or_default()
            },
            || fetch_merged_pr_refs(&project_root, pr_number, &ship_branch, &retarget_base),
        );
        if let Some(warning) = &outcome.warning {
            eprintln!(
                "  {} {warning}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
        slice_pull_skip = outcome.pull_skip;
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
    } else if let Some(reason) = seat_clone_post_merge_pull_skip_reason(&project_root)
        .or_else(|| slice_pull_skip.map(str::to_string))
    {
        // TASK-2053: both skip reasons are pure; the note and the log happen here.
        // trace:TASK-2053 trace:TASK-1951 | ai:claude
        eprintln!("  step 4: skipping `aida pull` ({reason})");
        log_ship_activity(
            &main_worktree,
            Some(pr_number),
            &ShipStep::Pull,
            &StepOutcome::Skipped(reason),
        );
    } else {
        eprintln!(
            "  step 4: running `aida pull` from main worktree ({})",
            main_worktree.display()
        );
        let pull_status = match prepare_main_worktree_for_pr_ship_pull(&main_worktree) {
            Ok(()) => {
                // Spawn `aida pull` as a subcommand. Use the hardened resolver
                // instead of a raw OS executable lookup: dev rebuilds can make Linux
                // report "<path> (deleted)", which Command::new cannot spawn.
                // trace:SPEC-411 | ai:codex
                let aida_bin = pr_ship_post_merge_aida_exe();
                let mut pull_cmd = std::process::Command::new(&aida_bin);
                pull_cmd.current_dir(&main_worktree).arg("pull");
                Some(
                    pull_cmd
                        .status_retrying_etxtbsy()
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
                let hint = recovery_hint(&ShipStep::Pull, Some(pr_number), forge_kind);
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
                let hint = recovery_hint(&ShipStep::Pull, Some(pr_number), forge_kind);
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
        // BUG-1953: match the PR head branch, not the cwd branch — shipping
        // by number from `main` must still end the shipped branch's lease.
        // `ship_branch` already falls back to `branch` when no PR branch
        // resolved. trace:BUG-1953 | ai:claude
        let lease_for_branch = list_leases(&main_worktree)
            .into_iter()
            .find(|l| l.branch == ship_branch);
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
                    // TASK-1582: name scope/role/worktree, not just the id.
                    // trace:TASK-1582 | ai:antigravity
                    let (line, detail) = pr_ship::step5_inside_worktree_messages(
                        &lease.id,
                        &lease.scope,
                        lease.role.as_deref(),
                        &lease.worktree_path,
                        &lease.branch,
                    );
                    eprintln!("{line}");
                    log_ship_activity(
                        &main_worktree,
                        Some(pr_number),
                        &ShipStep::EndLease,
                        &StepOutcome::Skipped(detail),
                    );
                } else {
                    eprintln!(
                        "  step 5: ending {}",
                        pr_ship::describe_lease_for_step5(
                            &lease.id,
                            &lease.scope,
                            lease.role.as_deref(),
                            &lease.worktree_path,
                        )
                    );
                    let aida_bin = pr_ship_post_merge_aida_exe();
                    let end_status = std::process::Command::new(&aida_bin)
                        .current_dir(&main_worktree)
                        .args(["session", "end", &lease.id, "--yes", "--skip-ci"])
                        .status_retrying_etxtbsy()
                        .context("could not invoke `aida session end`")?;
                    if !end_status.success() {
                        log_ship_activity(
                            &main_worktree,
                            Some(pr_number),
                            &ShipStep::EndLease,
                            &StepOutcome::Failed(format!("exit {}", end_status)),
                        );
                        let hint = recovery_hint(&ShipStep::EndLease, Some(pr_number), forge_kind);
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
                let (line, detail) = pr_ship::step5_no_lease_messages(&ship_branch);
                eprintln!("{line}");
                log_ship_activity(
                    &main_worktree,
                    Some(pr_number),
                    &ShipStep::EndLease,
                    &StepOutcome::Skipped(detail),
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
        // TASK-1177: reap first — a session whose spec is now finished, whose
        // branch just merged, and whose process has already exited is torn down
        // whole (worktree + lease + branch) instead of leaving a stranded
        // worktree for a human to clean up at the spec boundary. A session that
        // is still RUNNING is left completely alone; it reaps on a later pass
        // once it exits on its own. Quiet on the common no-op, and best-effort:
        // reaping must never turn a landed PR into a failed ship.
        // trace:TASK-1177 | ai:claude
        if let Err(e) = crate::session_reap::run_session_reap(crate::session_reap::ReapOptions {
            dry_run: false,
            yes: true,
            json: false,
            quiet_when_empty: true,
            release_unknown_owner: false,
            // trace:BUG-2197 | ai:claude — never release leases unconfirmed.
            release_protected: false,
        }) {
            eprintln!(
                "  {} post-merge session reap failed or was partially applied: {e:#}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
        eprintln!("  step 6: pruning verified merged agent worktrees");
        // trace:BUG-1718 | ai:codex
        if let Err(e) = doctor_cmd::run_merged_agent_worktree_gc_quiet() {
            eprintln!(
                "  {} post-merge worktree gc failed or was partially applied: {e:#}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
    }

    // BUG-1537: `merged_this_run` (set only where THIS invocation performed
    // the merge, never in the `already_merged` no-op path) is the one signal
    // that distinguishes an act this run performed from one it merely
    // observed already done. Report the two cases in first person only when
    // true — PRIN-5: a coordination surface must not render an absent act
    // identically to a performed one. trace:BUG-1537 | ai:claude
    if merged_this_run {
        eprintln!(
            "{} aida pr ship — PR-{} shipped",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
            pr_number
        );
    } else {
        eprintln!(
            "{} aida pr ship — PR-{} was already merged (not by this run); \
             ran post-merge sync + cleanup only",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
            pr_number
        );
    }

    // BUG-376: substrate-as-bouncer signal. The implementer's job ends
    // here — CI was gated in step 2, the merge ran in step 3, pull +
    // auto-bump in step 4, lease release in step 5 (any skipped step
    // already reported its alternate path above). The banner exists so
    // a confident LLM that "wants to helpfully watch CI" is told, at
    // the load-bearing moment, that there is nothing left to watch.
    // Paired with the `aida-implement.md` Step 7 skill directive. The
    // banner is the *substrate* half of the pairing — even an agent
    // that has not read or has misremembered the skill template sees
    // this on the way out. Printed only for a merge THIS run performed
    // (BUG-1537) — an already-merged PR gets the shorter honest
    // "observed merged" line instead, from the same function.
    // trace:BUG-376 trace:BUG-1537 | ai:claude
    let mut stderr = std::io::stderr();
    let _ = write_implementer_complete_banner(&mut stderr, pr_number, merged_this_run);

    // Keep the activity-event formatter referenced so the warning-as-
    // unused-import doesn't fire if a future refactor narrows usage.
    let _ = format_activity_event;
    Ok(())
}

/// Merge-hold markers are canonical in the main clone, even when `aida pr ship`
/// is launched from a sibling implementation worktree.
// trace:BUG-1188 | ai:codex
pub(crate) fn pr_ship_hold_root(project_root: &std::path::Path) -> std::path::PathBuf {
    main_worktree_root_from(project_root)
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
/// `merged_this_run` (BUG-1537) distinguishes a merge THIS invocation
/// performed from one it found already done (BUG-574's `already_merged`
/// no-op path, e.g. a sibling seat merged first). Rendering the two cases
/// identically is exactly the PRIN-5 defect BUG-1537 records: the loud
/// first-person "IMPLEMENTER COMPLETE" banner claims an act ("PR-N is
/// merged... ran in steps 2-5 above") that this run did not perform when
/// `false`. In that case we print a short, honest "observed merged" line
/// instead — still telling the session there is nothing left to do, without
/// claiming credit for the merge.
// trace:BUG-376 trace:BUG-1537 | ai:claude
pub(crate) fn write_implementer_complete_banner(
    w: &mut impl std::io::Write,
    pr_number: u64,
    merged_this_run: bool,
) -> std::io::Result<()> {
    if !merged_this_run {
        writeln!(w)?;
        writeln!(
            w,
            "  {} PR-{} was already merged by another run before this `aida pr \
             ship` started — this run only observed it and ran the idempotent \
             post-merge steps (pull / auto-bump, lease release). Nothing was \
             merged by this session; nothing left to watch or re-merge.",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            pr_number,
        )?;
        // BUG-376's exit directive is load-bearing for the implementer
        // session and must survive the honest wording (BUG-1537 review).
        // trace:BUG-1537 | ai:claude
        writeln!(
            w,
            "  {} — this implementer session has nothing left to do.",
            "EXIT NOW".bold()
        )?;
        writeln!(w)?;
        return Ok(());
    }
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
    // trace:TASK-2270 | ai:claude
    aida_core::git_ops::clear_sandbox_config_lock(project_root);
    let push_status = std::process::Command::new("git")
        .current_dir(project_root)
        .args(["push", "-u", "origin", branch])
        // trace:BUG-2181 | ai:claude
        .remote_credentials(project_root, "origin")
        .status()
        .context("could not invoke `git push`")?;
    if !push_status.success() {
        anyhow::bail!("`git push -u origin {branch}` failed — investigate before retrying");
    }
    aida_core::git_ops::repair_push_upstream(project_root, "origin", Some(branch), branch);

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
    let mut body = derive_pr_body_from_commit(&commit_msg);
    if title.is_empty() {
        anyhow::bail!("could not derive a non-empty PR title from the latest commit");
    }

    // TASK-1277: the typed-protocol check runs at PR creation too; its
    // per-item report lands in the PR body under a `## Protocol` heading, and
    // `[protocol] enforce = "refuse"` blocks creation unless the spec carries
    // the `--force` override ledgered at `aida queue done`.
    // trace:TASK-1277 | ai:claude
    if let Some(section) = protocol_section_for_commit(project_root, &commit_msg)? {
        if !body.trim().is_empty() {
            body.push_str("\n\n");
        }
        body.push_str(&section);
    }

    // TASK-1290: report (never block — the blocking/`--force`/ledger contract
    // lives at `aida queue done`, already run before a branch reaches PR
    // creation) untraced acceptance criteria for the spec(s) this PR's head
    // commit references. Reuses `aida criteria`'s tracer; quiet when clean.
    // trace:TASK-1290 | ai:claude
    report_untraced_criteria_for_commit(project_root, &commit_msg);

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
    let change = crate::forge::forge_for_open_change(project_root)
        .open_change(req)
        .context("could not open a forge change request")?;
    if change.id == 0 {
        anyhow::bail!(
            "`gh pr create` succeeded but no PR number found in its output: {}",
            change.url
        );
    }
    // STORY-1480: a PR opened interactively records the same PhaseDonePr the
    // drain's implementer phase records, so the timeline's work-done boundary
    // exists for interactively-shipped specs too. Specs credited from the
    // head commit's trailers; the feed lives at the MAIN worktree root.
    // trace:STORY-1480 | ai:claude
    crate::events::emit_interactive_lifecycle(
        &main_worktree_root_from(project_root),
        &extract_spec_ids_from_commit(&commit_msg),
        &crate::events::EventKind::PhaseDonePr {
            pr: change.id as u32,
        },
    );
    Ok(change.id)
}

/// TASK-1277: evaluate the type protocol's machine-checkable items for every
/// spec the head commit's `(SPEC-ID)` trailers name. Prints missing/unknown
/// lines, returns the `## Protocol` PR-body section, and errors (refusing PR
/// creation) when `[protocol] enforce = "refuse"` and a spec has a missing
/// item without a `--force` override ledger. Store/resolution failures are a
/// silent no-op — they never fabricate a pass or a refusal.
// trace:TASK-1277 | ai:claude
fn protocol_section_for_commit(
    project_root: &std::path::Path,
    commit_msg: &str,
) -> Result<Option<String>> {
    let spec_ids = extract_spec_ids_from_commit(commit_msg);
    if spec_ids.is_empty() {
        return Ok(None);
    }
    // A linked worktree usually has no `.aida-store/` of its own; fall back
    // to the main worktree's store rather than silently dropping the section.
    let store = match Storage::new(project_root.join(".aida-store")).load() {
        Ok(store) => store,
        Err(_) => {
            let main_root = main_worktree_root_from(project_root);
            match Storage::new(main_root.join(".aida-store")).load() {
                Ok(store) => store,
                Err(e) => {
                    eprintln!(
                        "  protocol: could not load the requirement store ({e:#}); \
                         protocol items not evaluated"
                    );
                    return Ok(None);
                }
            }
        }
    };
    let enforce = crate::criteria_gate::read_enforce_mode(project_root);
    let mut reports = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    for id in &spec_ids {
        let Some(req) = store.requirements.iter().find(|r| spec_matches(r, id)) else {
            continue;
        };
        let display = req.spec_id.as_deref().unwrap_or(id.as_str()).to_string();
        let report = crate::protocol_gate::evaluate_for_repo(req, project_root);
        if report.is_empty() {
            continue;
        }
        let forced = crate::protocol_gate::has_force_override(req);
        match crate::protocol_gate::decide(&report, enforce, forced) {
            crate::protocol_gate::GateOutcome::Proceed { unknown } => {
                for line in &unknown {
                    eprintln!("  {line}");
                }
            }
            crate::protocol_gate::GateOutcome::Warn {
                missing, unknown, ..
            } => {
                for line in unknown.iter().chain(missing.iter()) {
                    eprintln!(
                        "  {} {line}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                    );
                }
            }
            crate::protocol_gate::GateOutcome::Refuse { missing, unknown } => {
                for line in &unknown {
                    eprintln!("  {line}");
                }
                refused.extend(missing);
            }
        }
        reports.push((display, report));
    }
    if !refused.is_empty() {
        anyhow::bail!(
            "PR creation refused: missing protocol items ([protocol] enforce = \"refuse\"):\n  {}\n\
             Add the missing items, or override at `aida queue done --force` (ledgered).",
            refused.join("\n  ")
        );
    }
    Ok(crate::protocol_gate::pr_body_section(&reports))
}

/// TASK-1290: report-only (no blocking, no `--force`) pass over the spec(s)
/// named by `(SPEC-ID)` trailers in `commit_msg` — the same extraction
/// `generate_review_prompt` uses for `--pr`. Reuses `aida criteria`'s
/// tracer via `criteria_gate::evaluate`; a spec whose criteria are all
/// traced produces no output. Best-effort: any resolution failure (no
/// trailer, spec not found, store unreadable) is a silent no-op — PR
/// creation must never fail because this report couldn't run.
// trace:TASK-1290 | ai:claude
fn report_untraced_criteria_for_commit(project_root: &std::path::Path, commit_msg: &str) {
    let spec_ids = extract_spec_ids_from_commit(commit_msg);
    if spec_ids.is_empty() {
        return;
    }
    let storage = Storage::new(project_root.join(".aida-store"));
    let Ok(store) = storage.load() else {
        return;
    };
    for id in &spec_ids {
        let Some(req) = store.requirements.iter().find(|r| spec_matches(r, id)) else {
            continue;
        };
        let display = req.spec_id.as_deref().unwrap_or(id.as_str());
        let Ok(report) =
            crate::criteria::build_criteria_report(project_root, display, &req.description)
        else {
            continue;
        };
        if let crate::criteria_gate::CriteriaGate::Warn(lines) =
            crate::criteria_gate::evaluate(&report, crate::criteria_gate::EnforceMode::Warn)
        {
            eprintln!(
                "  {} {} has untraced acceptance criteria (no test traces them):",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                display
            );
            for line in &lines {
                eprintln!("    {line}");
            }
        }
    }
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
    let hint = crate::forge::resolve_open_change_forge_kind(probe_repo)
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
    match crate::forge::forge_for_open_change(probe_repo).open_change(req) {
        Ok(change) => {
            // STORY-1480: adopting unshipped work IS the integrate-side
            // implementer hand-off — before this, such a spec's history
            // read UnshippedWorkDetected → SpecCompleted with everything
            // between unknown. Record the adoption pair: the phase entry
            // (work existed; the adopter is picking it up) and the PR-open
            // boundary, both with empty run_uuid like every interactive
            // emit. trace:STORY-1480 | ai:claude
            let root = main_worktree_root_from(probe_repo);
            let specs = extract_spec_ids_from_commit(&commit_msg);
            crate::events::emit_interactive_lifecycle(
                &root,
                &specs,
                &crate::events::EventKind::PhaseEntered {
                    idx: 1,
                    slug: "implementer".to_string(),
                    vendor: None,
                    seat: None,
                    model: None,
                    effort: None,
                    attempt: 1,
                },
            );
            if change.id != 0 {
                crate::events::emit_interactive_lifecycle(
                    &root,
                    &specs,
                    &crate::events::EventKind::PhaseDonePr {
                        pr: change.id as u32,
                    },
                );
            }
            true
        }
        Err(e) => {
            eprintln!(
                "{} auto-open failed: {e:#}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
            );
            false
        }
    }
}

/// TASK-140: the full HEAD commit message of a BRANCH (not the local cwd HEAD).
/// Tries the local ref first, then `origin/<branch>` (shipping a PR whose branch
/// isn't checked out locally). `None` when neither resolves. Caller takes the
/// first line as the squash subject. trace:TASK-140 | ai:claude
pub(crate) fn branch_head_commit_message(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<String> {
    // A forge-reported PR head a fork author named. trace:BUG-1622 | ai:claude
    if crate::git_arg_guard::is_option_like(branch) {
        return None;
    }
    for r in [branch.to_string(), format!("origin/{branch}")] {
        let out = std::process::Command::new("git")
            .current_dir(project_root)
            .args([
                "log",
                "-1",
                "--format=%B",
                crate::git_arg_guard::END_OF_OPTIONS,
                &r,
                "--",
            ])
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
///
/// BUG-1919: also returns the completing trailer with its provenance, so the
/// caller can print where it came from (title / branch / withheld) before the
/// merge. `no_complete` is the `--no-complete` flag; the PR body's
/// non-completing marker and a closure-held spec (`closure:pending` or open
/// slice children) withhold the branch-name fallback inside the derivation.
// trace:BUG-1919 | ai:claude
pub(crate) fn derive_pr_ship_squash_subject(
    project_root: &std::path::Path,
    pr_number: u64,
    branch: &str,
    no_complete: bool,
) -> Result<(Option<String>, pr_ship::SquashTrailer)> {
    // TASK-140: compare against the PR BRANCH's head commit, NOT the local cwd
    // HEAD. `aida pr ship` frequently runs from the main worktree, where HEAD is
    // main's tip. TASK-142: prefer the PR title as the explicit squash subject
    // base because the branch head may itself be a merge commit.
    // trace:TASK-140 TASK-142 | ai:codex
    let pr = fetch_pr_ship_metadata_via_gh(project_root, pr_number)?;
    derive_pr_ship_squash_subject_from(project_root, &pr, branch, no_complete)
}

/// The derivation behind [`derive_pr_ship_squash_subject`], given the PR's
/// title and body already fetched, so tests drive it without a fake `gh` on
/// the process-wide PATH.
// trace:BUG-1997 | ai:claude
pub(crate) fn derive_pr_ship_squash_subject_from(
    project_root: &std::path::Path,
    pr: &PrShipMetadata,
    branch: &str,
    no_complete: bool,
) -> Result<(Option<String>, pr_ship::SquashTrailer)> {
    let commit_msg = branch_head_commit_message(project_root, branch).ok_or_else(|| {
        anyhow::anyhow!(
            "could not read the head commit of branch `{branch}` (tried local and origin/) \
             to derive the squash subject"
        )
    })?;
    derive_pr_ship_squash_subject_from_message(project_root, pr, branch, &commit_msg, no_complete)
}

/// [`derive_pr_ship_squash_subject_from`] given the head commit's message,
/// so a caller that holds the exact gated head (the merge train) reads the
/// subject at that commit, not at whatever a same-named local branch names.
// trace:TASK-2272 | ai:claude
pub(crate) fn derive_pr_ship_squash_subject_from_message(
    project_root: &std::path::Path,
    pr: &PrShipMetadata,
    branch: &str,
    commit_msg: &str,
    no_complete: bool,
) -> Result<(Option<String>, pr_ship::SquashTrailer)> {
    let current_subject = pr_ship::derive_pr_title_from_commit(&commit_msg);
    // A store read is required before granting an ID completion authority.
    // trace:TASK-1600 | ai:codex
    // This deliberately does NOT use `load_store_for_lookup` (the best-effort
    // loader the calibration and rework credit paths use, which yields `None`
    // on an unreadable store): here the store decides which specs the merge
    // completes, so an unreadable store must fail closed and stop the merge
    // rather than silently skipping id resolution and the closure hold.
    // trace:BUG-1957 | ai:claude
    let store_root = main_worktree_root_from(project_root);
    let store = Storage::new(store_root.join(".aida-store")).load()?;
    let (normalized, trailer) = pr_ship::derive_squash_subject_with_trailer(
        &pr.title,
        branch,
        &pr.body,
        commit_msg,
        no_complete,
        |id| matches!(store.get_requirement_unambiguous(id), Ok(Some(_))),
        |id| pr_ship::spec_completion_hold_reason(&store, id),
    )
    .unwrap_or_else(|| {
        (
            String::new(),
            pr_ship::SquashTrailer {
                ids: Vec::new(),
                source: pr_ship::TrailerSource::None,
                notes: Vec::new(),
            },
        )
    });
    // BUG-1997: never return None when spec IDs are withheld (via --no-complete,
    // a PR-body marker, or closure hold, or when the PR title carried an ID that
    // was stripped). Otherwise, gh pr merge defaults to the PR title on multi-commit
    // PRs and can carry the withheld ID into the squash subject on main.
    // trace:BUG-1997 | ai:antigravity
    let raw_title_ids = pr_ship::extract_trailing_spec_ids_from_subject(&pr.title);
    let title_ids_withheld = raw_title_ids.iter().any(|id| !trailer.ids.contains(id));
    let ids_withheld = no_complete || !trailer.notes.is_empty() || title_ids_withheld;
    if normalized.is_empty() || (normalized == current_subject && !ids_withheld) {
        return Ok((None, trailer));
    }
    Ok((Some(normalized), trailer))
}

/// STORY-1663: method-aware subject + completion-trailer derivation.
/// Squash merges derive both the squash subject and completion trailer;
/// rebase merges produce `None` for the squash subject and suppress spec
/// completion unless `final_slice` (`--final-slice` / `--complete-spec`) is set.
// trace:STORY-1663 | ai:antigravity
pub(crate) fn derive_pr_ship_subject_for_method(
    project_root: &std::path::Path,
    pr_number: u64,
    branch: &str,
    no_complete: bool,
    method: crate::forge::MergeMethod,
    final_slice: bool,
) -> Result<(Option<String>, pr_ship::SquashTrailer)> {
    if method != crate::forge::MergeMethod::Rebase {
        return derive_pr_ship_squash_subject(project_root, pr_number, branch, no_complete);
    }
    let pr = fetch_pr_ship_metadata_via_gh(project_root, pr_number)?;
    derive_pr_ship_subject_for_method_from(
        project_root,
        &pr,
        branch,
        no_complete,
        method,
        final_slice,
    )
}

/// Method-aware derivation given already-fetched `PrShipMetadata`.
// trace:STORY-1663 | ai:antigravity
pub(crate) fn derive_pr_ship_subject_for_method_from(
    project_root: &std::path::Path,
    pr: &PrShipMetadata,
    branch: &str,
    no_complete: bool,
    method: crate::forge::MergeMethod,
    final_slice: bool,
) -> Result<(Option<String>, pr_ship::SquashTrailer)> {
    if method != crate::forge::MergeMethod::Rebase {
        return derive_pr_ship_squash_subject_from(project_root, pr, branch, no_complete);
    }
    let store_root = main_worktree_root_from(project_root);
    let store = Storage::new(store_root.join(".aida-store")).load()?;
    let trailer = ship_completion_trailer_for_method(
        &store,
        method,
        final_slice,
        &pr.title,
        branch,
        &pr.body,
        no_complete,
    );
    Ok((None, trailer))
}

/// The `aida pr ship --dry-run` output: the step plan followed by the
/// completion-trailer provenance line, so the operator sees which specs the
/// merge would complete, and why, without merging. `pr_number == 0` means
/// the PR would be created, so there is no title to derive from yet.
// trace:BUG-1957 | ai:claude
pub(crate) fn pr_ship_dry_run_output(
    plan: String,
    project_root: &std::path::Path,
    pr_number: u64,
    ship_branch: &str,
    no_complete: bool,
) -> String {
    pr_ship_dry_run_output_for_method(
        plan,
        project_root,
        pr_number,
        ship_branch,
        no_complete,
        crate::forge::MergeMethod::Squash,
        false,
    )
}

/// Method-aware dry-run output for `aida pr ship --dry-run`.
// trace:STORY-1663 | ai:antigravity
pub(crate) fn pr_ship_dry_run_output_for_method(
    plan: String,
    project_root: &std::path::Path,
    pr_number: u64,
    ship_branch: &str,
    no_complete: bool,
    method: crate::forge::MergeMethod,
    final_slice: bool,
) -> String {
    let provenance = if pr_number == 0 {
        pr_ship::format_dry_run_trailer_provenance(Err(
            "the PR does not exist yet; it is derived from the PR title or branch name at merge time",
        ))
    } else {
        match derive_pr_ship_subject_for_method(
            project_root,
            pr_number,
            ship_branch,
            no_complete,
            method,
            final_slice,
        ) {
            Ok((_, trailer)) => pr_ship::format_dry_run_trailer_provenance(Ok(&trailer)),
            Err(e) => pr_ship::format_dry_run_trailer_provenance(Err(&e.to_string())),
        }
    };
    plan + &provenance
}

/// BUG-1919: the one store-qualified completion-trailer derivation the ship
/// path uses for the post-merge credit (calibration slots, the `PrMerged`
/// event), so it cannot disagree with the squash subject about which specs
/// the PR completed. Resolution and closure-hold both read `store`.
// trace:BUG-1919 | ai:claude
pub(crate) fn ship_completion_trailer(
    store: &aida_core::RequirementsStore,
    pr_title: &str,
    branch: &str,
    pr_body: &str,
    no_complete: bool,
) -> pr_ship::SquashTrailer {
    ship_completion_trailer_for_method(
        store,
        crate::forge::MergeMethod::Squash,
        false,
        pr_title,
        branch,
        pr_body,
        no_complete,
    )
}

/// STORY-1663: method-aware store-qualified completion trailer derivation.
/// Rebase merges suppress spec completion unless `final_slice` (`--final-slice`
/// / `--complete-spec`) is set.
// trace:STORY-1663 | ai:antigravity
pub(crate) fn ship_completion_trailer_for_method(
    store: &aida_core::RequirementsStore,
    method: crate::forge::MergeMethod,
    final_slice: bool,
    pr_title: &str,
    branch: &str,
    pr_body: &str,
    no_complete: bool,
) -> pr_ship::SquashTrailer {
    pr_ship::derive_ship_trailer_for_method(
        method,
        final_slice,
        pr_title,
        branch,
        pr_body,
        no_complete,
        |id| matches!(store.get_requirement_unambiguous(id), Ok(Some(_))),
        |id| pr_ship::spec_completion_hold_reason(store, id),
    )
}

/// TASK-1765: `body` with an `aida: no-complete` declaration line appended,
/// so the squash commit on main carries the non-completing marker the pull
/// reads (`commit_declares_non_completing`).
// trace:TASK-1951 trace:TASK-1765 | ai:claude
pub(crate) fn non_completing_squash_body(body: &str) -> String {
    let body = body.trim_end();
    let marker = pr_ship::NO_COMPLETE_BODY_MARKERS[0];
    if body.is_empty() {
        marker.to_string()
    } else {
        format!("{body}\n\n{marker}")
    }
}

/// The squash body `aida pr ship` hands the forge. A non-completing squash
/// ship carries the `aida: no-complete` declaration, appended to the
/// identity gate's sanitized body or, when the gate left the body to the
/// forge, to the branch's commit messages (`commit_messages`).
///
/// TASK-2116 trade-off: an explicit body replaces the forge's default one,
/// so GitHub no longer derives `Co-authored-by:` trailers from the other
/// commit authors for that merge. Trailers already written in commit
/// messages are kept. A non-completing ship accepts this so the declaration
/// reaches main; an ordinary ship leaves the body to the forge.
// trace:TASK-1951 trace:TASK-1765 trace:TASK-2116 trace:TASK-2117 | ai:claude
pub(crate) fn ship_squash_body(
    method: crate::forge::MergeMethod,
    declares_non_completing: bool,
    sanitized: Option<String>,
    commit_messages: impl FnOnce() -> Result<String>,
) -> Result<Option<String>> {
    if method != crate::forge::MergeMethod::Squash || !declares_non_completing {
        return Ok(sanitized);
    }
    let body = match sanitized {
        Some(body) => body,
        None => commit_messages()?,
    };
    Ok(Some(non_completing_squash_body(&body)))
}

/// STORY-1663: optional slice-stack verifier hook before rebase merge.
/// Reads `scripts/split-lib/verify_stack.py` from `base_ref`, the base the PR
/// merges into. The `main_worktree` copy is used only when `base_ref` does not
/// resolve (a local-only repository); a base that resolves without the script
/// has no verifier. When the verifier is present, fails
/// closed if the commit range `base_ref..head_ref` cannot be inspected, and
/// invokes `python3 <verifier> <base_ref> <head_ref>` when any commit in the
/// range matches `refactor(cli-lib): extract|extend <m>.rs`.
// trace:STORY-1663 | ai:antigravity
pub(crate) fn run_optional_slice_stack_verifier(
    project_root: &std::path::Path,
    main_worktree: &std::path::Path,
    pr_number: u64,
    base_ref: &str,
    head_ref: &str,
) -> Result<()> {
    // trace:TASK-1951 trace:TASK-1619 | ai:claude
    let base_present = std::process::Command::new("git")
        .current_dir(project_root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{base_ref}^{{commit}}"),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let origin_blob = if base_present {
        let out = std::process::Command::new("git")
            .current_dir(project_root)
            .args([
                "show",
                &format!("{base_ref}:scripts/split-lib/verify_stack.py"),
            ])
            .output()
            .with_context(|| format!("could not read verify_stack.py from `{base_ref}`"))?;
        if !out.status.success() || out.stdout.is_empty() {
            return Ok(());
        }
        Some(out.stdout)
    } else {
        None
    };
    let fallback_path = main_worktree.join("scripts/split-lib/verify_stack.py");
    if origin_blob.is_none() && !fallback_path.is_file() {
        return Ok(());
    }

    // Ensure `head_ref` is resolvable locally when shipping by PR number from `main`.
    let head_commit = format!("{head_ref}^{{commit}}");
    let head_present = std::process::Command::new("git")
        .current_dir(project_root)
        .args(["rev-parse", "--verify", "--quiet", &head_commit])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !head_present {
        let _ = std::process::Command::new("git")
            .current_dir(project_root)
            .args([
                "fetch",
                "--quiet",
                "origin",
                &format!("refs/pull/{pr_number}/head"),
            ])
            // trace:BUG-2181 | ai:claude
            .remote_credentials(project_root, "origin")
            .output();
        let _ = std::process::Command::new("git")
            .current_dir(project_root)
            .args(["fetch", "--quiet", "origin", head_ref])
            // trace:BUG-2181 | ai:claude
            .remote_credentials(project_root, "origin")
            .output();
    }

    let range = format!("{base_ref}..{head_ref}");
    let log_out = match std::process::Command::new("git")
        .current_dir(project_root)
        .args(["log", "--format=%s", &range])
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            let refusal = format!(
                "PR-{pr_number} rebase ship refused: could not inspect commit range `{range}` for slice-stack verification ({e})"
            );
            return Err(pr_ship::ShipRefusal::Review.error(refusal));
        }
    };
    if !log_out.status.success() {
        let stderr = String::from_utf8_lossy(&log_out.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("exit {}", log_out.status)
        } else {
            stderr
        };
        let refusal = format!(
            "PR-{pr_number} rebase ship refused: could not inspect commit range `{range}` for slice-stack verification ({detail})"
        );
        return Err(pr_ship::ShipRefusal::Review.error(refusal));
    }
    let subjects = String::from_utf8_lossy(&log_out.stdout);
    let has_slice_commits = subjects.lines().any(pr_ship::is_slice_stack_commit_subject);
    if !has_slice_commits {
        return Ok(());
    }

    let mut temp_verifier: Option<tempfile::NamedTempFile> = None;
    let verifier_path = if let Some(bytes) = origin_blob {
        use std::io::Write;
        let mut tmp = tempfile::Builder::new()
            .prefix("aida-verify-stack-")
            .suffix(".py")
            .tempfile()
            .context("could not create temporary file for the base's verify_stack.py")?;
        tmp.write_all(&bytes)
            .context("could not write the base's verify_stack.py")?;
        tmp.flush()
            .context("could not flush the base's verify_stack.py")?;
        let path = tmp.path().to_path_buf();
        temp_verifier = Some(tmp);
        path
    } else {
        fallback_path
    };

    let output = std::process::Command::new("python3")
        .current_dir(project_root)
        .arg(&verifier_path)
        .arg(base_ref)
        .arg(head_ref)
        .output()
        .context("failed to run scripts/split-lib/verify_stack.py")?;
    drop(temp_verifier);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("exit {}", output.status)
        };
        let refusal = format!(
            "PR-{pr_number} rebase ship refused: slice-stack verifier (`scripts/split-lib/verify_stack.py {base_ref} {head_ref}`) failed ({detail})"
        );
        return Err(pr_ship::ShipRefusal::Review.error(refusal));
    }
    Ok(())
}

/// TASK-1621: whether the rebase slice bookkeeping (final-slice completion or
/// the non-completing marker) runs: for a merge this run made, and for a
/// re-run that finds the PR already merged.
// trace:TASK-1951 trace:TASK-1621 | ai:claude
pub(crate) fn rebase_slice_bookkeeping_due(merged_this_run: bool, already_merged: bool) -> bool {
    merged_this_run || already_merged
}

/// TASK-1622: why step 4 skips `aida pull` after the merge, beyond
/// `--no-pull`: an unrecorded non-completing marker would let the pull
/// complete the slice's spec from a rebased `(SPEC-ID)` subject.
// trace:TASK-1951 trace:TASK-1622 | ai:claude
pub(crate) fn post_merge_pull_skip_reason(slice_marker_failed: bool) -> Option<&'static str> {
    slice_marker_failed.then_some("non-completing slice marker not recorded")
}

/// The head branch, base and head sha a merged PR was shipped from, and the
/// subjects of its commits (newest first) when the forge reported them.
// trace:TASK-1951 trace:TASK-1621 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RebaseSliceInputs {
    pub(crate) branch: String,
    pub(crate) base: String,
    pub(crate) head_sha: Option<String>,
    /// Empty: read them from the local `base..head` range.
    pub(crate) subjects: Vec<String>,
}

/// TASK-1621: the slice bookkeeping inputs. A merge made in this run uses the
/// values ship resolved for the open PR. An already-merged re-run resolves
/// them from the merged PR (`merged_refs`), because the open-PR lookup misses
/// a merged PR and the checkout ship runs from is usually `main`; when they
/// cannot be resolved it refuses rather than guess.
// trace:TASK-1951 trace:TASK-1621 | ai:claude
pub(crate) fn rebase_slice_inputs(
    already_merged: bool,
    pr_number: u64,
    ship_branch: &str,
    retarget_base: &str,
    merged_head_sha: Option<&str>,
    merged_refs: impl FnOnce() -> Result<RebaseSliceInputs>,
) -> Result<RebaseSliceInputs> {
    if !already_merged {
        return Ok(RebaseSliceInputs {
            branch: ship_branch.to_owned(),
            base: retarget_base.to_owned(),
            head_sha: merged_head_sha.map(str::to_owned),
            subjects: Vec::new(),
        });
    }
    merged_refs().with_context(|| {
        format!("could not resolve the merged PR-{pr_number}'s head branch, base and head sha")
    })
}

/// The merged change's refs from the forge-neutral change metadata, plus
/// its commit subjects. Refuses a change that is not merged, or a branch
/// name or sha that is unsafe to pass to git.
// trace:TASK-1951 trace:TASK-1621 trace:TASK-2131 | ai:claude
pub(crate) fn merged_pr_refs_from_metadata(
    meta: &crate::forge::ChangeMetadata,
    headlines: Vec<String>,
) -> Result<RebaseSliceInputs> {
    anyhow::ensure!(
        meta.state == crate::forge::ChangeState::Merged,
        "the PR is not merged (state `{:?}`)",
        meta.state
    );
    let (branch, base, sha) = (
        meta.head_ref.trim(),
        meta.base_ref.trim(),
        meta.head_sha.trim(),
    );
    anyhow::ensure!(
        !branch.is_empty() && !base.is_empty() && !sha.is_empty(),
        "the forge reported no head branch, base or head sha"
    );
    anyhow::ensure!(
        crate::git_arg_guard::is_shell_safe_branch_name(branch)
            && crate::git_arg_guard::is_shell_safe_branch_name(base)
            && crate::git_arg_guard::is_hex_sha(sha),
        "the forge returned an unsafe branch name or sha"
    );
    let mut subjects: Vec<String> = headlines
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    // Forges list a change's commits oldest first; the marker reads newest first.
    subjects.reverse();
    Ok(RebaseSliceInputs {
        branch: branch.to_owned(),
        base: base.to_owned(),
        head_sha: Some(sha.to_owned()),
        subjects,
    })
}

/// TASK-2131: the merged PR's refs through the forge abstraction, so an
/// already-merged re-run works on GitLab as on GitHub. Pure-git has no
/// merged-change record: the slice branch named on the command line is the
/// record, so the re-run must name it, and its commits are read from the
/// local `<base>..<slice branch>` range (see [`pure_git_slice_refs`]).
// trace:TASK-1951 trace:TASK-1621 trace:TASK-2131 | ai:claude
fn fetch_merged_pr_refs(
    project_root: &std::path::Path,
    n: u64,
    ship_branch: &str,
    base: &str,
) -> Result<RebaseSliceInputs> {
    let forge = crate::forge::forge_for(project_root);
    if forge.kind() == crate::forge::ForgeKind::None {
        anyhow::ensure!(
            ship_branch != base && crate::git_arg_guard::is_shell_safe_branch_name(ship_branch),
            "a pure-git project has no merged change record; re-run `aida pr ship --rebase` from the slice branch"
        );
        return pure_git_slice_refs(project_root, ship_branch, base);
    }
    let mut sink = crate::network_retry::StderrSink;
    let meta = forge.change_metadata(n, &mut sink)?;
    let headlines = forge.change_commit_headlines(n, &mut sink)?;
    merged_pr_refs_from_metadata(&meta, headlines)
}

/// TASK-2131, review round 1: the pure-git already-merged inputs. A rebase
/// merge rewrites the slice's commits onto the base, so the slice branch
/// still holds the originals and `<base>..<slice branch>` lists their
/// subjects, newest first, which [`confirm_rebase_merged`] then finds on the
/// base. `origin/<base>` is fetched first and the newer of it and the local
/// base is read, so a stale tracking ref cannot count base commits as the
/// slice's (TASK-2251). A fast-forward merge leaves the range empty; its
/// commits are then read from the slice's fork point (see
/// [`fast_forward_slice_fork`]). A branch that never had commits is refused,
/// because there is nothing to confirm.
// trace:TASK-1951 trace:TASK-2131 trace:TASK-2251 | ai:claude
fn pure_git_slice_refs(
    project_root: &std::path::Path,
    ship_branch: &str,
    base: &str,
) -> Result<RebaseSliceInputs> {
    let git_out = |args: &[&str]| -> Option<String> {
        aida_core::git_ops::host_git_command(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let head = git_out(&["rev-parse", "--verify", "--quiet", ship_branch])
        .map(|out| out.trim().to_owned())
        .filter(|sha| !sha.is_empty())
        .with_context(|| format!("the slice branch `{ship_branch}` does not exist locally"))?;
    // No `origin` remote (a local-only pure-git project): nothing to fetch.
    // trace:TASK-2251 | ai:claude
    let has_origin = git_out(&["remote", "get-url", "origin"]).is_some();
    let fetched = has_origin
        && aida_core::git_ops::host_git_command(project_root)
            .args(["fetch", "--quiet", "origin", base])
            .remote_credentials(project_root, "origin")
            .output()
            .is_ok_and(|out| out.status.success());
    if has_origin && !fetched {
        eprintln!(
            "  warning: could not fetch origin/{base}; reading the slice range from the newer of the local refs"
        );
    }
    let base_ref = freshest_base_ref(&git_out, base)
        .with_context(|| format!("neither `origin/{base}` nor `{base}` exists locally"))?;
    let subjects_in = |range: String| -> Vec<String> {
        git_out(&["log", "--format=%s", &range])
            .map(|out| {
                out.lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut subjects = subjects_in(format!("{base_ref}..{head}"));
    if subjects.is_empty() {
        if let Some(fork) = fast_forward_slice_fork(&git_out, ship_branch, &base_ref, &head) {
            subjects = subjects_in(format!("{fork}..{head}"));
        }
    }
    anyhow::ensure!(
        !subjects.is_empty(),
        "`{base}..{ship_branch}` holds no commits, and the branch's reflog shows none it was fast-forwarded with, so the rebase merge of `{ship_branch}` cannot be confirmed in a pure-git project"
    );
    Ok(RebaseSliceInputs {
        branch: ship_branch.to_owned(),
        base: base.to_owned(),
        head_sha: Some(head),
        subjects,
    })
}

/// TASK-2251 (TASK-2249): the base ref a pure-git slice range is read from:
/// the local base when it contains `origin/<base>` (it is at least as new),
/// else `origin/<base>`, else the local base. `None` when neither exists.
// trace:TASK-2251 | ai:claude
fn freshest_base_ref(git_out: &dyn Fn(&[&str]) -> Option<String>, base: &str) -> Option<String> {
    let exists = |r: &str| {
        git_out(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{r}^{{commit}}"),
        ])
        .is_some()
    };
    let remote = format!("origin/{base}");
    match (exists(&remote), exists(base)) {
        (true, true) => {
            let local_is_newer = git_out(&["merge-base", "--is-ancestor", &remote, base]).is_some();
            Some(if local_is_newer {
                base.to_owned()
            } else {
                remote
            })
        }
        (true, false) => Some(remote),
        (false, true) => Some(base.to_owned()),
        (false, false) => None,
    }
}

/// TASK-2251 (TASK-2248): where a fast-forwarded slice forked from the base.
/// A fast-forward puts the slice head on the base, so `<base>..<head>` is
/// empty. The fork comes from the slice's own reflog ([`authored_slice_start`]):
/// where the branch was created, or where its latest rebase replayed it onto.
/// When the base's reflog shows it stood later than that before it moved onto
/// the head, that position is the fork. `None` when the head is not on the
/// base, the reflog does not show the branch authoring its own tip, or the
/// fork is the head itself.
// trace:TASK-2251 | ai:claude
fn fast_forward_slice_fork(
    git_out: &dyn Fn(&[&str]) -> Option<String>,
    ship_branch: &str,
    base_ref: &str,
    head: &str,
) -> Option<String> {
    let is_ancestor = |a: &str, b: &str| git_out(&["merge-base", "--is-ancestor", a, b]).is_some();
    if !is_ancestor(head, base_ref) {
        return None;
    }
    let reflog = |r: &str| -> Vec<(String, String)> {
        git_out(&["log", "-g", "--format=%H %gs", r])
            .map(|out| {
                out.lines()
                    .filter_map(|l| l.trim().split_once(' '))
                    .map(|(sha, msg)| (sha.to_owned(), msg.to_owned()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let start = authored_slice_start(
        &reflog(&format!("refs/heads/{ship_branch}")),
        ship_branch,
        head,
    )?;
    let base_before = reflog(base_ref)
        .into_iter()
        .map(|(sha, _)| sha)
        .find(|sha| !is_ancestor(head, sha));
    let fork = match base_before {
        Some(before) if is_ancestor(&start, &before) && is_ancestor(&before, head) => before,
        _ => start,
    };
    (fork != head && is_ancestor(&fork, head)).then_some(fork)
}

/// TASK-2251 review round 1: fail closed. The slice's reflog (newest first)
/// must show the branch created, then only entries that author its tip: a
/// commit, an amend, a cherry-pick, or a rebase that replayed commits onto
/// a base (its new tip is not the `onto` commit itself). Any other entry —
/// a fast-forward merge or pull, a reset, a rebase that picked nothing —
/// may have moved the tip onto commits others made, so nothing is
/// confirmed. Returns where the slice's own commits start: the `onto` of its
/// latest rebase, else its creation point.
// trace:TASK-2251 | ai:claude
fn authored_slice_start(
    branch_log: &[(String, String)],
    ship_branch: &str,
    head: &str,
) -> Option<String> {
    let ((created, created_msg), authored) = branch_log.split_last()?;
    if !created_msg.starts_with("branch: Created from") || authored.is_empty() {
        return None;
    }
    if authored.first().map(|(sha, _)| sha.as_str()) != Some(head) {
        return None;
    }
    let rebase_marker = format!("(finish): refs/heads/{ship_branch} onto ");
    let mut start = created.clone();
    for (sha, msg) in authored.iter().rev() {
        let authoring = ["commit: ", "commit (amend): ", "cherry-pick: "]
            .iter()
            .any(|prefix| msg.starts_with(prefix));
        let onto = (msg.starts_with("rebase") || msg.starts_with("pull"))
            .then(|| msg.split_once(rebase_marker.as_str()))
            .flatten()
            .map(|(_, onto)| onto.trim())
            .filter(|onto| !onto.is_empty() && onto != sha);
        match onto {
            Some(onto) => start = onto.to_owned(),
            None if authoring => {}
            None => return None,
        }
    }
    Some(start)
}

/// What a run of `aida pr ship` needs to do the rebase slice bookkeeping.
// trace:TASK-1951 trace:TASK-1621 | ai:claude
pub(crate) struct RebaseSliceRun<'a> {
    pub(crate) main_worktree: &'a std::path::Path,
    pub(crate) project_root: &'a std::path::Path,
    pub(crate) pr_number: u64,
    pub(crate) final_slice: bool,
    pub(crate) already_merged: bool,
    pub(crate) ship_branch: &'a str,
    pub(crate) retarget_base: &'a str,
    pub(crate) merged_head_sha: Option<&'a str>,
}

/// The result of the rebase slice bookkeeping: a warning to print, and the
/// reason step 4 must skip `aida pull`, if any.
// trace:TASK-1951 trace:TASK-1622 | ai:claude
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SliceBookkeeping {
    pub(crate) warning: Option<String>,
    pub(crate) pull_skip: Option<&'static str>,
}

/// STORY-1663: rebase merges have no squash subject, so `aida pull`'s
/// commit-subject auto-bump cannot tell a final-slice rebase merge from an
/// intermediate slice. With `--final-slice` the resolved specs are marked
/// Completed; without it a non-completing marker is recorded so `aida pull`
/// leaves the owning spec unchanged.
///
/// TASK-1621: it also runs on a re-run that finds the PR already merged, so
/// a crash between the merge and this step is repaired by re-running
/// `aida pr ship N --rebase`. That path reads the branch, base, head and
/// commits from the merged PR, never from the checkout, and the specs a
/// final slice completes come from the merged branch (`spec_ids_for`).
/// TASK-2130: it also confirms the PR really was rebase-merged (its commits
/// sit on the base as one run); a squash or merge-commit merge records and
/// completes nothing. Without the marker the pull is skipped (TASK-1622).
// trace:STORY-1663 | ai:antigravity
// trace:TASK-1951 trace:TASK-1621 trace:TASK-1622 trace:TASK-2130 | ai:claude
pub(crate) fn rebase_slice_bookkeeping(
    run: &RebaseSliceRun<'_>,
    this_run_spec_ids: &[String],
    spec_ids_for: impl FnOnce(&str) -> Vec<String>,
    merged_refs: impl FnOnce() -> Result<RebaseSliceInputs>,
) -> SliceBookkeeping {
    let pr = run.pr_number;
    let inputs = rebase_slice_inputs(
        run.already_merged,
        pr,
        run.ship_branch,
        run.retarget_base,
        run.merged_head_sha,
        merged_refs,
    )
    .and_then(|inputs| {
        if run.already_merged {
            confirm_rebase_merged(run.project_root, pr, &inputs)?;
        }
        Ok(inputs)
    });
    if run.final_slice {
        return match inputs {
            Ok(inputs) => {
                let ids = if run.already_merged {
                    spec_ids_for(&inputs.branch)
                } else {
                    this_run_spec_ids.to_vec()
                };
                complete_rebase_final_slice_specs(
                    run.main_worktree,
                    pr,
                    &ids,
                    inputs.head_sha.as_deref(),
                );
                SliceBookkeeping::default()
            }
            Err(e) => SliceBookkeeping {
                warning: Some(format!(
                    "--final-slice completed nothing for PR-{pr}: {e:#}"
                )),
                pull_skip: None,
            },
        };
    }
    match inputs.and_then(|inputs| {
        record_non_completing_rebase_slice(
            run.main_worktree,
            run.project_root,
            pr,
            &inputs.base,
            inputs.head_sha.as_deref(),
            &inputs.branch,
            &inputs.subjects,
        )
    }) {
        Ok(()) => SliceBookkeeping::default(),
        Err(e) => SliceBookkeeping {
            warning: Some(format!(
                "could not record the non-completing slice marker for PR-{pr}: {e:#}\n    skipping `aida pull` so no spec completes; re-run `aida pr ship {pr} --rebase`"
            )),
            pull_skip: post_merge_pull_skip_reason(true),
        },
    }
}

/// TASK-2130: an already-merged re-run takes `--rebase` from the operator,
/// not from the forge. Confirm the PR was rebase-merged: its commits must sit
/// on the base as one consecutive run. A squash merge leaves one commit with
/// another subject, and a merge commit keeps the original commits off the
/// first-parent line, so neither confirms.
// trace:TASK-1951 trace:TASK-2130 | ai:claude
fn confirm_rebase_merged(
    project_root: &std::path::Path,
    pr: u64,
    inputs: &RebaseSliceInputs,
) -> Result<()> {
    anyhow::ensure!(
        !inputs.subjects.is_empty(),
        "the forge reported no commits for PR-{pr}, so its rebase merge cannot be confirmed"
    );
    let landed = landed_commits_for_subjects(
        project_root,
        &inputs.base,
        inputs.head_sha.as_deref(),
        &inputs.subjects,
    );
    anyhow::ensure!(
        landed.iter().all(Option::is_some),
        "PR-{pr} was not rebase-merged: its {} commit(s) do not sit on `{}` as one run. \
         A squash or merge-commit merge has no slice bookkeeping, so nothing was recorded or completed",
        inputs.subjects.len(),
        inputs.base
    );
    Ok(())
}

/// STORY-1663: record a `[aida:non-completing-rebase]` marker comment on any
/// spec referenced by a rebase-merged slice PR when `--final-slice` was not
/// passed, so `aida pull` and `aida db reconcile-status` leave the spec's
/// status unchanged even if a commit subject carried `(SPEC-ID)`.
///
/// The marker names the commits as they LANDED on the base (`landed: <sha>`):
/// a rebase merge rewrites them, so the PR head's sha never reaches main, and
/// a subject alone would also suppress a later commit that reuses it. Only a
/// subject that cannot be found on the base is recorded as a `subject:` line.
/// Every failure is returned, never swallowed.
// trace:STORY-1663 | ai:antigravity
// trace:TASK-1951 trace:TASK-1620 trace:TASK-1622 | ai:claude
pub(crate) fn record_non_completing_rebase_slice(
    main_worktree: &std::path::Path,
    project_root: &std::path::Path,
    pr_number: u64,
    base_branch: &str,
    merged_head_sha: Option<&str>,
    ship_branch: &str,
    known_subjects: &[String],
) -> Result<()> {
    let head_ref = merged_head_sha
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(ship_branch);
    // Deferral (b) of #2816: subjects the forge reported for the merged PR
    // are used as they are, so a clone that never fetched the PR head can
    // still write the marker. trace:TASK-1951 trace:TASK-1621 | ai:claude
    let mut range_subjects: Vec<String> = known_subjects.to_vec();
    for base_candidate in [format!("origin/{base_branch}"), base_branch.to_string()] {
        if !range_subjects.is_empty() {
            break;
        }
        let range = format!("{base_candidate}..{head_ref}");
        if let Ok(out) = std::process::Command::new("git")
            .current_dir(project_root)
            .args(["log", "--format=%s", &range])
            .output()
        {
            if out.status.success() {
                range_subjects = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                break;
            }
        }
    }
    if range_subjects.is_empty() {
        if let Ok(out) = std::process::Command::new("git")
            .current_dir(project_root)
            .args(["show", "-s", "--format=%s", head_ref])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    range_subjects.push(s);
                }
            }
        }
    }

    let mut candidate_specs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for id in pr_ship::extract_spec_ids_from_text(ship_branch) {
        candidate_specs.insert(id);
    }
    for subj in &range_subjects {
        for id in crate::extract_spec_ids_from_commit(subj) {
            candidate_specs.insert(id);
        }
    }
    // An unreadable range is a failure even when no spec id was found yet:
    // the missing subjects are where the ids would have come from.
    // trace:TASK-1951 trace:TASK-1622 | ai:claude
    if range_subjects.is_empty() {
        anyhow::bail!("could not read the commit subjects of `{head_ref}`");
    }
    if candidate_specs.is_empty() {
        return Ok(());
    }

    let store_path = main_worktree.join(".aida-store");
    if !store_path.is_dir() {
        anyhow::bail!("no store at {}", store_path.display());
    }
    let backend = aida_core::CachedGitBackend::open(
        &store_path,
        &aida_core::CachedGitBackend::default_cache_path(&store_path),
    )
    .with_context(|| format!("could not open the store at {}", store_path.display()))?;
    use aida_core::DatabaseBackend;
    let landed =
        landed_commits_for_subjects(project_root, base_branch, merged_head_sha, &range_subjects);
    let header = format!(
        "{} PR #{} rebase-merged without --final-slice; status stays unchanged until the final slice completes.",
        pr_ship::NON_COMPLETING_REBASE_MARKER,
        pr_number
    );
    let mut comment_lines = vec![header.clone()];
    if let Some(sha) = merged_head_sha.filter(|s| !s.trim().is_empty()) {
        comment_lines.push(format!("head: {}", sha.trim()));
    }
    for (subj, landed_sha) in range_subjects.iter().zip(&landed) {
        match landed_sha {
            Some(sha) => comment_lines.push(format!("landed: {sha}")),
            None => comment_lines.push(format!("subject: {subj}")),
        }
    }
    let body = comment_lines.join("\n");
    let now = chrono::Utc::now();
    let mut failures: Vec<String> = Vec::new();
    for spec_id in candidate_specs {
        let req = match backend.get_requirement_unambiguous(&spec_id) {
            Ok(Some(req)) => req,
            // A branch or subject token that names no spec has nothing to mark.
            Ok(None) => continue,
            Err(e) => {
                failures.push(format!("{spec_id}: {e}"));
                continue;
            }
        };
        if let Err(e) = backend.update_spec_atomically(&req, |r| {
            // One marker per PR: a re-run that resolves slightly different
            // lines never appends a second one. trace:TASK-1951 | ai:claude
            let already = r
                .comments
                .iter()
                .any(|c| c.content.lines().next() == Some(header.as_str()));
            if !already {
                r.add_comment(aida_core::Comment::new(
                    "aida-pr-ship".to_string(),
                    body.clone(),
                ));
                r.modified_at = now;
            }
        }) {
            failures.push(format!("{spec_id}: {e}"));
        }
    }
    if !failures.is_empty() {
        anyhow::bail!("marker not written for {}", failures.join("; "));
    }
    Ok(())
}

/// TASK-1620: the sha each range commit landed as on `base_branch`, aligned
/// with `subjects` (both newest first). A rebase merge puts the commits on
/// the base's first-parent line as ONE consecutive run, so only a run that
/// matches every subject in order counts: a later commit that reuses one
/// slice subject cannot be bound in place of the real one (TASK-2132), and a
/// stale `origin/<base>` that does not hold the run yet matches nothing
/// instead of something wrong (#2816 deferral (c)). The remote tracking ref
/// (after a fetch) is searched first, then the local branch; the run may be
/// in either commit order. No run leaves every entry `None`.
///
/// Review round 1 (TASK-2132): with the PR's head known, the walk stops at
/// the PR's fork point (see [`slice_fork_point`]) and the OLDEST matching run
/// above it is taken, so a later commit that reuses the subject (n = 1, or a
/// whole run repeated later) cannot bind in place of the slice. Without a
/// head, the newest run is taken as before.
// trace:TASK-1951 trace:TASK-1620 trace:TASK-2132 | ai:claude
fn landed_commits_for_subjects(
    project_root: &std::path::Path,
    base_branch: &str,
    head_sha: Option<&str>,
    subjects: &[String],
) -> Vec<Option<String>> {
    let fetched = std::process::Command::new("git")
        .current_dir(project_root)
        .args(["fetch", "--quiet", "origin", base_branch])
        // trace:BUG-2181 | ai:claude
        .remote_credentials(project_root, "origin")
        .output()
        .is_ok_and(|out| out.status.success());
    if !fetched {
        eprintln!(
            "  warning: could not fetch origin/{base_branch}; looking for the landed slice commits in the local refs"
        );
    }
    if subjects.is_empty() {
        return Vec::new();
    }
    for base in [format!("origin/{base_branch}"), base_branch.to_string()] {
        let fork = head_sha
            .filter(|h| !h.trim().is_empty())
            .and_then(|head| slice_fork_point(project_root, &base, head.trim(), subjects.len()));
        let range = match &fork {
            Some(fork) => format!("{fork}..{base}"),
            None => base.clone(),
        };
        let Some(out) = std::process::Command::new("git")
            .current_dir(project_root)
            .args([
                "log",
                "--first-parent",
                "-n",
                "500",
                "--format=%H%x09%s",
                &range,
            ])
            .output()
            .ok()
            .filter(|out| out.status.success())
        else {
            continue;
        };
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let log: Vec<(&str, &str)> = stdout
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(sha, subject)| (sha, subject.trim()))
            .collect();
        if let Some(run) = landed_run(&log, subjects, fork.is_some()) {
            return run.into_iter().map(Some).collect();
        }
    }
    vec![None; subjects.len()]
}

/// TASK-2132: the base commit the PR's run landed on top of, the lower bound
/// of the landed-run walk. A rebase merge rewrites the commits, so the PR head
/// stays off the base and its merge-base with the base is the fork point. A
/// fast-forward leaves the head on the base, so the fork point is `n` commits
/// below it. `None` (head unknown or unreachable) leaves the walk unbounded.
// trace:TASK-1951 trace:TASK-2132 | ai:claude
fn slice_fork_point(
    project_root: &std::path::Path,
    base: &str,
    head: &str,
    n: usize,
) -> Option<String> {
    let git_out = |args: &[&str]| -> Option<String> {
        aida_core::git_ops::host_git_command(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    let head = git_out(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("{head}^{{commit}}"),
    ])?;
    let fork = git_out(&["merge-base", base, &head])?;
    if fork != head {
        return Some(fork);
    }
    git_out(&["rev-parse", "--verify", "--quiet", &format!("{head}~{n}")])
}

/// A consecutive run of `log` (newest first) whose subjects are `subjects`
/// in order, or in reverse order; the shas are returned aligned with
/// `subjects`. `oldest` takes the oldest such run (the walk is bounded below
/// at the PR's fork point, so that is the slice itself); otherwise the newest.
// trace:TASK-1951 trace:TASK-2132 | ai:claude
fn landed_run(log: &[(&str, &str)], subjects: &[String], oldest: bool) -> Option<Vec<String>> {
    let n = subjects.len();
    if n == 0 || log.len() < n {
        return None;
    }
    let mut found = None;
    for start in 0..=log.len() - n {
        let window = &log[start..start + n];
        let run = if window.iter().zip(subjects).all(|((_, s), want)| s == want) {
            Some(window.iter().map(|(sha, _)| sha.to_string()).collect())
        } else if window
            .iter()
            .rev()
            .zip(subjects)
            .all(|((_, s), want)| s == want)
        {
            Some(
                window
                    .iter()
                    .rev()
                    .map(|(sha, _)| sha.to_string())
                    .collect(),
            )
        } else {
            None
        };
        if run.is_some() {
            found = run;
            if !oldest {
                break;
            }
        }
    }
    found
}

/// STORY-1663: transition resolved specs to `Completed` when a rebase merge
/// ships with `--final-slice` (`--complete-spec`).
// trace:STORY-1663 | ai:antigravity
pub(crate) fn complete_rebase_final_slice_specs(
    main_worktree: &std::path::Path,
    pr_number: u64,
    spec_ids: &[String],
    merged_head_sha: Option<&str>,
) {
    if spec_ids.is_empty() {
        return;
    }
    let store_path = main_worktree.join(".aida-store");
    let backend = match aida_core::CachedGitBackend::open(
        &store_path,
        &aida_core::CachedGitBackend::default_cache_path(&store_path),
    ) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "  {} could not open store at {} to complete {:?} after PR-{pr_number}: {e}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                store_path.display(),
                spec_ids
            );
            return;
        }
    };
    use aida_core::DatabaseBackend;
    let sha = merged_head_sha.unwrap_or("");
    let now = chrono::Utc::now();
    let author = current_user_id(None);
    for spec_id in spec_ids {
        let req = match backend.get_requirement_unambiguous(spec_id) {
            Ok(Some(req)) => req,
            Ok(None) => {
                eprintln!(
                    "  {} could not complete {spec_id} after PR-{pr_number}: spec not found in store",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
                continue;
            }
            Err(e) => {
                eprintln!(
                    "  {} could not resolve {spec_id} to complete after PR-{pr_number}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
                continue;
            }
        };
        let display_id = req.display_id();
        match crate::completion::transition_to_completed_atomically(
            &backend,
            &req,
            Some(main_worktree),
            &display_id,
            sha,
            "pr-ship-rebase",
            |r, prior| {
                aida_core::conflict::record_status_transition(r, &author, prior);
                r.modified_at = now;
                if !sha.is_empty() {
                    r.implementation_info
                        .get_or_insert_with(aida_core::ImplementationInfo::default)
                        .completion_sha = Some(sha.to_string());
                }
            },
        ) {
            Ok(crate::completion::AtomicCompletion::Completed) => {
                eprintln!(
                    "  {} completed {} via final-slice rebase merge of PR-{}",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    display_id,
                    pr_number
                );
            }
            Ok(crate::completion::AtomicCompletion::AlreadyCompleted) => {
                eprintln!(
                    "  {} {} is already Completed (PR-{})",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    display_id,
                    pr_number
                );
            }
            Ok(crate::completion::AtomicCompletion::Refused(status)) => {
                eprintln!(
                    "  {} could not complete {} after PR-{}: spec is in terminal status {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    display_id,
                    pr_number,
                    status
                );
            }
            Ok(crate::completion::AtomicCompletion::Gone) => {
                eprintln!(
                    "  {} could not complete {} after PR-{}: spec disappeared before write",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    display_id,
                    pr_number
                );
            }
            Err(e) => {
                eprintln!(
                    "  {} could not complete {} after final-slice rebase merge of PR-{}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    display_id,
                    pr_number
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShipDefaultBranchDisposition {
    FeatureBranch,
    OnDefaultBranch,
    UnknownDefaultBranch,
}

/// Protect conventional mainlines; otherwise require evidence of the default.
/// Unknown evidence is reported distinctly from being on the default branch.
// trace:TASK-1795 trace:TASK-1803 | ai:antigravity
pub(crate) fn classify_ship_default_branch(
    project_root: &std::path::Path,
    branch: &str,
) -> ShipDefaultBranchDisposition {
    if matches!(branch, "main" | "master") {
        return ShipDefaultBranchDisposition::OnDefaultBranch;
    }
    // Only origin/HEAD is local evidence, never a guess from leftover refs.
    // A stale origin/HEAD still wins over the forge, so refresh it when the
    // remote default changes (git remote set-head origin --auto).
    // trace:TASK-1951 | ai:claude
    let local = crate::forge::local_origin_head_branch(project_root);
    match local.or_else(|| crate::forge::probe_default_branch(project_root)) {
        Some(default) if branch == default => ShipDefaultBranchDisposition::OnDefaultBranch,
        Some(_) => ShipDefaultBranchDisposition::FeatureBranch,
        None => ShipDefaultBranchDisposition::UnknownDefaultBranch,
    }
}

// Test-only since classify_ship_default_branch replaced it on the ship path.
// trace:TASK-1795 trace:TASK-1803 | ai:antigravity
// trace:TASK-1951 trace:TASK-1408 | ai:claude
#[cfg(test)]
pub(crate) fn ship_is_default_branch(project_root: &std::path::Path, branch: &str) -> bool {
    !matches!(
        classify_ship_default_branch(project_root, branch),
        ShipDefaultBranchDisposition::FeatureBranch
    )
}

/// BUG-2164: Query forge for open approved PRs. Gracefully returns empty vec on any error.
// trace:BUG-2164 | ai:antigravity
pub(crate) fn find_open_approved_prs(
    project_root: &std::path::Path,
) -> Vec<pr_ship::OpenApprovedPr> {
    if crate::forge::resolve_forge_kind(project_root) != crate::forge::ForgeKind::GitHub {
        return Vec::new();
    }
    let gh_bin = match crate::kernel::resolve_gh_binary() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let mut cmd = std::process::Command::new(&gh_bin);
    cmd.current_dir(project_root).args([
        "pr",
        "list",
        "--state",
        "open",
        "--limit",
        "50",
        "--json",
        "number,title,reviewDecision",
    ]);
    let Some(out) = crate::kernel::command_output_with_timeout(cmd, crate::FORGE_CLI_CALL_TIMEOUT)
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    pr_ship::parse_open_approved_prs_json(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
#[path = "tests/bug_2164_default_branch_ship_refusal_tests.rs"]
mod bug_2164_default_branch_ship_refusal_tests;

#[cfg(test)]
#[path = "tests/task_140_squash_subject_tests.rs"]
mod task_140_squash_subject_tests;

#[cfg(test)]
#[path = "tests/bug_2203_refs_subject_tests.rs"]
mod bug_2203_refs_subject_tests;

/// A ship a mandate covered that stops short of its merge is a
/// mandate-driven action too: ledger its refusal under the mandate. Every
/// exit between the mandate release decision and the merge calls this (a
/// source pin keeps it so); a ship no mandate covered writes nothing.
// trace:TASK-1915 | ai:claude
pub(crate) fn ledger_mandate_ship_failure(
    root: &std::path::Path,
    cover: Option<&MandateCover>,
    pr: u64,
    why: &str,
) {
    if let Some(cover) = cover {
        crate::mandate_drive::emit_session_ship(
            root,
            &cover.id,
            pr,
            crate::mandate_drive::Outcome::Refused,
            Some(why.to_string()),
        );
    }
}

#[cfg(test)]
#[path = "tests/task_1915_mandate_ship_failure_ledger_tests.rs"]
mod task_1915_mandate_ship_failure_ledger_tests;

pub(crate) struct PrShipMetadata {
    pub(crate) title: String,
    pub(crate) body: String,
}

pub(crate) fn fetch_pr_ship_metadata_via_gh(
    project_root: &std::path::Path,
    n: u64,
) -> Result<PrShipMetadata> {
    let n_str = n.to_string();
    let mut command = std::process::Command::new("gh");
    command
        .current_dir(project_root)
        .args(["pr", "view", &n_str, "--json", "title,body"]);
    let out = crate::process_retry::command_output_retrying_etxtbsy(&mut command).map_err(|e| {
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

// trace:BUG-344 | ai:codex
/// STORY-1166: test harness — the production path is `Forge::checks_registered`
/// (GitHub impl) driven by `ci_gate::wait_for_checks_to_register`; this keeps
/// the fake-`gh` tests over the shared classifier so the two cannot drift.
#[cfg(test)]
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
        let out = crate::process_retry::command_output_retrying_etxtbsy(&mut command)
            .with_context(|| format!("could not invoke `gh pr checks {pr_number}`"))?;
        if pr_ship::classify_gh_pr_checks_registration(
            pr_number,
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
            out.status.success(),
        )? == crate::forge::CheckRegistration::Registered
        {
            return Ok(());
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

    // trace:BUG-1985 | ai:claude
    let status = aida_core::git_ops::host_git_command(main_worktree)
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

/// Why step 4 skips `aida pull` when ship runs from a contained seat's
/// private clone: the main checkout is read-only to the seat, so the host
/// syncs it (reap, the heartbeat, or the operator's next `aida pull`). Pure;
/// the ship step prints and logs the reason.
// trace:TASK-2053 | ai:claude
pub(crate) fn seat_clone_post_merge_pull_skip_reason(
    project_root: &std::path::Path,
) -> Option<String> {
    let clone = aida_core::seat_clone_detect::seat_clone_root(project_root)?;
    Some(format!(
        "seat clone {}: the main checkout is synced from outside the seat; run `aida pull` there",
        clone.display()
    ))
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
    append_ship_activity_line(main_worktree, &line);
}

/// Append one pre-formatted JSON line to `.aida/advisor-activity.jsonl`
/// (best-effort, like every activity-log write).
// trace:TASK-1458 | ai:claude
pub(crate) fn append_ship_activity_line(main_worktree: &std::path::Path, line: &str) {
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

// trace:BUG-1037 | ai:codex
pub(crate) fn preflight_stale_base_check_with_forge(
    project_root: &std::path::Path,
    n: u64,
    forge_kind: crate::forge::ForgeKind,
) -> Result<pr_rebase::StaleBaseOutcome> {
    let info = fetch_change_info_via_resolved_forge(project_root, n, forge_kind)?;
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
        // trace:BUG-2181 | ai:claude
        .remote_credentials(&project_root, "origin")
        .status();
    // BUG-1218: reviewer probes must not update the conventional pr-N branch:
    // a stale/sibling worktree may have it checked out. A per-process scratch
    // ref is safe to fetch and is deleted after the probe.
    // trace:BUG-1218 | ai:codex
    let pr_local_branch = format!("refs/aida/preflight/pr-{n}-{}", std::process::id());
    // BUG-1228: forge-aware head ref (refs/merge-requests/N/head on GitLab).
    let pr_head_ref = crate::pr_head_remote_ref(project_root, n);
    let pr_refspec = format!("+{pr_head_ref}:{pr_local_branch}");
    let pr_fetch = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["fetch", "origin", pr_refspec.as_str()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // trace:BUG-2181 | ai:claude
        .remote_credentials(project_root, "origin")
        .status();
    if !matches!(pr_fetch, Ok(s) if s.success()) {
        anyhow::bail!(
            "could not fetch PR-{n}'s head ref (`{pr_head_ref}`) for pre-flight \
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

    let outcome = classify_stale_base(behind, &pr_files, &base_files, prediction);
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["update-ref", "-d", &pr_local_branch])
        .status();
    Ok(outcome)
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

// trace:BUG-1037 | ai:codex
pub(crate) fn preflight_intermediate_only_check_with_forge(
    project_root: &std::path::Path,
    n: u64,
    forge_kind: crate::forge::ForgeKind,
) -> Result<pr_rebase::IntermediateOnlyOutcome> {
    let info = fetch_change_info_via_resolved_forge(project_root, n, forge_kind)?;
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
        // trace:BUG-2181 | ai:claude
        .remote_credentials(&project_root, "origin")
        .status();
    // trace:BUG-1218 | ai:codex
    let pr_local_branch = format!("refs/aida/preflight/pr-{n}-{}", std::process::id());
    // BUG-1228: forge-aware head ref (refs/merge-requests/N/head on GitLab).
    let pr_head_ref = crate::pr_head_remote_ref(project_root, n);
    let pr_refspec = format!("+{pr_head_ref}:{pr_local_branch}");
    let pr_fetch = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["fetch", "origin", pr_refspec.as_str()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // trace:BUG-2181 | ai:claude
        .remote_credentials(project_root, "origin")
        .status();
    if !matches!(pr_fetch, Ok(s) if s.success()) {
        anyhow::bail!(
            "could not fetch PR-{n}'s head ref (`{pr_head_ref}`) for pre-flight \
             intermediate-only check"
        );
    }

    let changed = files_in_range(project_root, &format!("{origin_base}..{pr_local_branch}"));
    let outcome = classify_intermediate_only_with_gitignore(project_root, &changed);
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["update-ref", "-d", &pr_local_branch])
        .status();
    Ok(outcome)
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

// ---------------------------------------------------------------------------
// BUG-1610: pre-MR-creation base check + `--target-branch`-overridable
// recovery state.
//
// A GitLab project whose remote `main` is never pushed lets the FIRST pushed
// branch become the project's default. `aida review <SPEC>` then offers to
// open a change from that branch with no explicit base — `glab mr create
// --source-branch <branch>` (no `--target-branch`) silently targets the
// project default, i.e. the SAME branch, and GitLab later refuses to merge
// it ("You must be on a different branch other than <branch>"). Diagnosing
// this BEFORE offering the create command turns a confusing later `glab`
// failure into an immediate, actionable refusal naming both branches.
// trace:BUG-1610 | ai:claude

/// Outcome of checking the intended base before offering to create a
/// change.
// trace:BUG-1610 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MrBasePreflight {
    /// The base exists on the remote and differs from the source — safe to
    /// offer.
    Ok,
    /// `origin` has no `refs/heads/<base>` — the intended base was never
    /// pushed.
    MissingRemoteBase,
    /// The base and the source branch are the same ref.
    SourceEqualsBase,
    /// `origin` could not be reached (offline, auth, hung connection), so
    /// the base could not be verified either way. Fails closed: no offer.
    OriginUnreachable,
}

/// Check whether `base` is a safe MR/PR target for `source_branch`: it must
/// exist on `origin` and must not be the same ref as the source. Only talks
/// to the remote (`git ls-remote`) — no local fetch required, so it is
/// correct even against a bare fixture `origin` with nothing fetched yet.
// trace:BUG-1610 | ai:claude
pub(crate) fn preflight_mr_base(
    project_root: &std::path::Path,
    source_branch: &str,
    base: &str,
) -> MrBasePreflight {
    if base == source_branch {
        return MrBasePreflight::SourceEqualsBase;
    }
    // Bounded the same way as `probe_branch_on_origin` (BUG-257): a hung
    // HTTPS dial must not freeze `aida review`, and no credential prompt may
    // block it. `--exit-code` exits 2 only when origin answered and has no
    // such ref; any other failure means origin was not reached.
    // trace:BUG-1610 | ai:claude
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=10",
            "ls-remote",
            "--exit-code",
            "--heads",
            crate::git_arg_guard::END_OF_OPTIONS, // trace:BUG-1622 | ai:claude
            "origin",
            base,
        ])
        // trace:BUG-2181 | ai:claude
        .remote_credentials(project_root, "origin")
        .output();
    match out {
        Ok(o) if o.status.success() && !o.stdout.is_empty() => MrBasePreflight::Ok,
        Ok(o) if o.status.code() == Some(2) => MrBasePreflight::MissingRemoteBase,
        _ => MrBasePreflight::OriginUnreachable,
    }
}

/// Refusal text for a non-`Ok` [`MrBasePreflight`] — names both branches and
/// gives the exact, provider-correct recovery sequence instead of letting
/// the caller offer an impossible change-create.
// trace:BUG-1610 | ai:claude
pub(crate) fn mr_base_diagnosis_message(
    forge_kind: crate::forge::ForgeKind,
    outcome: MrBasePreflight,
    source_branch: &str,
    base: &str,
) -> String {
    let change_noun = forge_kind.change_noun();
    let condition = match outcome {
        MrBasePreflight::Ok => return String::new(),
        MrBasePreflight::OriginUnreachable => {
            return format!(
                "not offering a {change_noun} from `{source_branch}`: could not reach \
                 `origin` to confirm that `{base}` exists there. Check the network or \
                 credentials and run the command again."
            );
        }
        MrBasePreflight::MissingRemoteBase => format!(
            "the intended base `{base}` does not exist on `origin`. This usually \
             happens when the repository was initialized before `{base}` was pushed, \
             so the forge made `{source_branch}` the project's default branch instead"
        ),
        MrBasePreflight::SourceEqualsBase => format!(
            "it is also the intended base `{base}` (the project's current default \
             branch) — a change cannot merge into itself"
        ),
    };
    let mut msg = format!(
        "refusing to offer a {change_noun} from `{source_branch}` — {condition}.\n\n\
         Recovery:\n  1. git push -u origin {base}\n"
    );
    match forge_kind {
        crate::forge::ForgeKind::GitLab => {
            msg.push_str(&format!(
                "  2. glab repo update --defaultBranch {base}\n  \
                 3. glab mr create --source-branch {source_branch} --target-branch {base}"
            ));
        }
        crate::forge::ForgeKind::GitHub => {
            msg.push_str(&format!(
                "  2. gh repo edit --default-branch {base}\n  \
                 3. gh pr create --head {source_branch} --base {base}"
            ));
        }
        crate::forge::ForgeKind::None => {
            msg.push_str("  2. open the change against that base through your forge's UI");
        }
    }
    msg
}

/// Persisted state of a change offered/attempted via `aida review`'s AC-5
/// (re)open-a-change prompt — enough to remember which base a retry should
/// target without the operator re-typing it every time.
// trace:BUG-1610 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MrRecoveryState {
    pub spec: String,
    pub source_branch: String,
    pub target_branch: String,
}

/// `.aida/mr-recovery/<spec>.json` under the project root — mirrors
/// [`crate::punt::hold_signal_path`]'s convention for small, spec-scoped JSON
/// markers.
// trace:BUG-1610 | ai:claude
pub(crate) fn mr_recovery_path(project_root: &std::path::Path, spec: &str) -> std::path::PathBuf {
    project_root
        .join(".aida")
        .join("mr-recovery")
        .join(format!("{spec}.json"))
}

/// Write a recovery-state marker, creating `.aida/mr-recovery/` if needed.
///
/// Refuses (returns `Err`, writes nothing) when `source_branch ==
/// target_branch` — persisting that pair is exactly the self-referential
/// state BUG-1610 observed a hand-run `glab mr create --source-branch
/// <branch>` (no `--target-branch`) leave behind when the branch was also
/// the project's default. A later, corrected `--target-branch` retry must
/// never read that broken pair back (see [`resolve_mr_target_branch`]), so
/// it must never be written in the first place — enforced here rather than
/// trusted to every call site.
// trace:BUG-1610 | ai:claude
pub(crate) fn write_mr_recovery_state(
    path: &std::path::Path,
    state: &MrRecoveryState,
) -> Result<()> {
    if state.source_branch == state.target_branch {
        anyhow::bail!(
            "refusing to save MR recovery state for {} with source == target (`{}`)",
            state.spec,
            state.source_branch
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

/// Read a recovery-state marker. `None` when absent or unparseable — either
/// way the caller falls back to the resolved default base.
// trace:BUG-1610 | ai:claude
pub(crate) fn read_mr_recovery_state(path: &std::path::Path) -> Option<MrRecoveryState> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

/// Resolve the target branch for a (re)tried change-create. An explicit
/// `--target-branch` ALWAYS wins over whatever a previous attempt saved —
/// a corrected retry must never be silently overridden by stale recovery
/// state (the exact BUG-1610 failure: `--recover --target-branch main`
/// still failing because a saved `story-52-work -> story-52-work` pair won).
// trace:BUG-1610 | ai:claude
pub(crate) fn resolve_mr_target_branch(
    explicit_target_branch: Option<&str>,
    saved: Option<&MrRecoveryState>,
    default_base: &str,
) -> String {
    if let Some(t) = explicit_target_branch {
        return t.to_string();
    }
    if let Some(s) = saved {
        return s.target_branch.clone();
    }
    default_base.to_string()
}

#[cfg(test)]
#[path = "tests/bug_1610_mr_base_recovery_tests.rs"]
mod bug_1610_mr_base_recovery_tests;

#[cfg(test)]
#[path = "tests/task_471_stale_base_preflight_tests.rs"]
mod task_471_stale_base_preflight_tests;

#[cfg(test)]
#[path = "tests/story_429_auto_rebase_tests.rs"]
mod story_429_auto_rebase_tests;

#[cfg(test)]
#[path = "tests/task1312_pr_gc_tests.rs"]
mod task1312_pr_gc_tests;

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
        let out = crate::test_git::git_command()
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
    fn recover_open_change_uses_github_origin_even_with_stale_pure_git_config() {
        // trace:BUG-1994 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        // trace:BUG-896 | ai:codex
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        init_repo(&repo);
        git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/repo.git",
            ],
        );
        std::fs::create_dir_all(repo.join(".aida")).unwrap();
        std::fs::write(
            repo.join(".aida").join("config.toml"),
            "[forge]\nprovider = \"pure-git\"\n",
        )
        .unwrap();
        git(&repo, &["checkout", "-b", "bug-896", "--quiet"]);
        std::fs::write(repo.join("work.txt"), "work\n").unwrap();
        git(&repo, &["add", "work.txt"]);
        git(
            &repo,
            &[
                "commit",
                "-m",
                "[AI:codex] fix(test): recover open change (BUG-896)",
                "--quiet",
            ],
        );

        let marker = tmp.path().join("gh-args");
        let gh = bin.join("gh");
        make_executable(
            &gh,
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = repo ] && [ \"$2\" = view ]; then\n\
                   echo main\n\
                   exit 0\n\
                 fi\n\
                 printf '%s\\n' \"$*\" > '{}'\n\
                 test \"$1\" = pr\n\
                 test \"$2\" = create\n\
                 echo 'https://github.com/acme/repo/pull/896'\n",
                marker.display()
            ),
        );
        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let mut path_entries = vec![bin.clone()];
        path_entries.extend(std::env::split_paths(&old_path));
        let path = std::env::join_paths(path_entries).unwrap();
        let _env = crate::test_env::EnvVarGuard::set("PATH", path);

        assert!(recover_open_change(&repo, "bug-896"));
        let args = std::fs::read_to_string(marker).unwrap();
        assert!(args.contains("pr create"), "{args}");
        assert!(args.contains("--base main"), "{args}");
        assert!(args.contains("--head bug-896"), "{args}");

        // STORY-1480: the adoption recorded its lifecycle pair — the
        // implementer entry and the PR-open boundary — credited to the spec
        // the head commit's trailer names, with no run uuid (no drain).
        // trace:STORY-1480 | ai:claude
        let feed = crate::events::read_all(&repo);
        let adoption: Vec<_> = feed
            .iter()
            .filter(|e| e.spec.as_deref() == Some("BUG-896"))
            .collect();
        assert_eq!(adoption.len(), 2, "{adoption:?}");
        assert!(matches!(
            adoption[0].kind,
            crate::events::EventKind::PhaseEntered { ref slug, attempt: 1, .. } if slug == "implementer"
        ));
        assert!(matches!(
            adoption[1].kind,
            crate::events::EventKind::PhaseDonePr { pr: 896 }
        ));
        assert!(adoption.iter().all(|e| e.run_uuid.is_empty()));
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
        let merge_args = pr_ship::merge_args(732, delete_branch, None, None);
        assert!(!merge_args.iter().any(|arg| arg == "--delete-branch"));
    }

    #[test]
    fn pr_ship_reads_merge_hold_from_main_worktree_when_run_from_sibling() {
        // trace:BUG-1188 | ai:codex
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let sibling = tmp.path().join("bug-1188-worktree");
        git(
            tmp.path(),
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "bug-1188",
                sibling.to_str().unwrap(),
            ],
        );

        crate::merge_hold::write_hold(
            tmp.path(),
            1188,
            "BUG-1188 is marked drive — merge requires review",
        )
        .unwrap();

        let hold_root = pr_ship_hold_root(&sibling);
        // trace:BUG-1192 | ai:codex
        assert_eq!(
            hold_root.canonicalize().unwrap(),
            tmp.path().canonicalize().unwrap()
        );
        assert!(
            crate::merge_hold::read_hold(&hold_root, 1188)
                .unwrap()
                .contains("BUG-1188"),
            "pr ship launched from a sibling worktree must see the main clone's hold marker"
        );
        // BUG-2176: a read keyed by the sibling resolves to the same primary
        // marker — there is no per-worktree hold store left to read empty.
        // trace:BUG-2176 | ai:claude
        assert!(
            crate::merge_hold::read_hold(&sibling, 1188).is_some_and(|r| r.contains("BUG-1188")),
            "a sibling-rooted read must see the main clone's hold marker"
        );
    }

    // BUG-1423: `aida pr ship` merging a PR outside a drain phase must append
    // the same `PrMerged` event the drain's merge phase does — one per
    // credited spec, carrying the acting seat when known. This is the
    // decisive gap the bug measured: nine of ten advisor merges recorded
    // nothing.
    #[test]
    fn emit_ship_pr_merged_writes_one_event_per_credited_spec_with_seat() {
        let tmp = tempfile::tempdir().unwrap();
        let _seat = crate::test_env::AmbientGuard::hermetic_with_seat(tmp.path(), "advisor", &[]);

        emit_ship_pr_merged(
            tmp.path(),
            4242,
            &["BUG-1423".to_string(), "TASK-1".to_string()],
        );

        let events = crate::events::read_all(tmp.path());
        assert_eq!(events.len(), 2, "one PrMerged per credited spec");
        for (ev, expected_spec) in events.iter().zip(["BUG-1423", "TASK-1"]) {
            assert!(
                matches!(ev.kind, crate::events::EventKind::PrMerged { pr } if pr == 4242),
                "expected PrMerged{{pr: 4242}}, got {:?}",
                ev.kind
            );
            assert_eq!(ev.spec.as_deref(), Some(expected_spec));
            assert_eq!(
                ev.seat.as_deref(),
                Some("advisor"),
                "the acting seat must ride along — this is the field the \
                 PrMerged undercount hid: an advisor merge and a drain merge \
                 must be distinguishable in the feed"
            );
        }
    }

    // BUG-1423: a PR crediting no resolvable spec (title/branch/body all
    // miss) must still emit — the merge itself is never silently dropped
    // from the feed just because spec attribution failed.
    #[test]
    fn emit_ship_pr_merged_emits_once_with_no_spec_when_none_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let _seat = crate::test_env::EnvVarGuard::unset("AIDA_SESSION_ROLE");

        emit_ship_pr_merged(tmp.path(), 99, &[]);

        let events = crate::events::read_all(tmp.path());
        assert_eq!(events.len(), 1);
        assert!(events[0].spec.is_none());
        assert!(events[0].seat.is_none());
        assert!(matches!(
            events[0].kind,
            crate::events::EventKind::PrMerged { pr: 99 }
        ));
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
    let outcome = queue_review_for_open_pr(&project_root, &branch);

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

/// The one shared file-the-reviewer-story step for a PR that is open on
/// `branch`: file (or skip, if already filed) the review story, render the
/// outcome, clear a satisfied PR-hold marker, bump the covered specs to In
/// Progress, and on every skip print the exact re-run command. `aida pr
/// auto-queue-review` and `aida ship --no-merge` (BUG-2217) both run it, so
/// a PR opened by either path gets its reviewer story whatever the spec
/// status — the auto-queue has no status gate.
// trace:BUG-2217 | ai:claude
pub(crate) fn queue_review_for_open_pr(
    project_root: &std::path::Path,
    branch: &str,
) -> crate::AutoQueueOutcome {
    // Try to associate this run with the active session lease so the
    // review-story description records who opened the PR. Falls back to
    // a synthetic "(no-session)" id if no lease covers the cwd.
    let lease = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(project_root, &cwd));
    let lease_scope = lease.as_ref().map(|l| l.scope.clone());
    let session_id = lease
        .map(|l| l.id)
        .unwrap_or_else(|| "(no-sess)".to_string());

    let outcome =
        try_auto_queue_pr_review(project_root, branch, &session_id, AutoQueueOrigin::PrSkill);
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
            let main_root =
                find_main_worktree_root().unwrap_or_else(|_| project_root.to_path_buf());
            let marker = punt::hold_signal_path(&main_root, spec);
            if punt::read_hold_signal(&marker).is_some() {
                let _ = std::fs::remove_file(&marker);
            }
        }
        // BUG-1944: `try_auto_queue_pr_review` already moved the branch's and
        // covered specs to In Progress; the session scope is known only here.
        // trace:BUG-1944 | ai:claude
        if let Some(spec) = lease_scope
            .as_deref()
            .and_then(crate::worktree_lease::spec_id_from_branch)
        {
            let main_root =
                find_main_worktree_root().unwrap_or_else(|_| project_root.to_path_buf());
            crate::live_work::bump_spec_in_progress(
                &main_root,
                &spec,
                aida_core::conflict::PR_OPEN_AUTHOR,
            );
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

    outcome
}

/// BUG-1944: the specs an opened PR starts work on — the session's spec scope,
/// the spec named by the branch, and the specs the PR covers — deduplicated,
/// upper-cased, non-spec scopes dropped.
// trace:BUG-1944 | ai:claude
pub(crate) fn pr_open_bump_targets(
    lease_scope: Option<&str>,
    branch: &str,
    covered_specs: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let candidates = lease_scope
        .and_then(crate::worktree_lease::spec_id_from_branch)
        .into_iter()
        .chain(crate::worktree_lease::spec_id_from_branch(branch))
        .chain(
            covered_specs
                .iter()
                .filter_map(|s| crate::worktree_lease::spec_id_from_branch(s)),
        );
    for id in candidates {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
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

/// TASK-1458: the `aida pr ship` approval gate as `pr_ship_handler` runs it —
/// the merge pin it returns and the durable activity records it writes.
// trace:TASK-1458 | ai:claude
#[cfg(test)]
mod task_1458_pr_ship_approval_gate_tests {
    use super::pr_ship_approval_gate;
    use crate::pr_ship::ShipApprovalPolicy;
    use crate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};

    const HEAD: &str = "1aca4e3e9251aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OLD: &str = "08834c6045a9bbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    /// When HEAD was pushed in the stale cases: AFTER the approval of OLD.
    const HEAD_COMMITTED_AT: &str = "2026-09-24T00:00:00Z";

    fn approval(sha: &str) -> RecordedVerdict {
        RecordedVerdict {
            kind: VerdictKind::Approved,
            raw: "approved".into(),
            reviewed_sha: Some(sha.into()),
            recorded_at: Some("2026-09-23T00:00:00Z".into()),
            // BUG-1918: an independent human-at-TTY recorder, so these tests
            // exercise the head-coverage gate behind the authority gate.
            // trace:BUG-1918 | ai:claude
            attestation: Some(RecorderAttestation {
                sha: sha.into(),
                human_at_tty: true,
                receipt_id: Some("rcpt".into()),
                identity: vec!["session:independent-reviewer".into()],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// BUG-1918: a policy under which `verdicts`' attestations re-validated.
    /// Realistic dates: the REVIEWED commit was committed (09-22) before the
    /// approval (09-23); the head of a stale case was pushed after it
    /// (09-24, see [`HEAD_COMMITTED_AT`]) — which is exactly why the
    /// backdating check compares against the reviewed commit, not the head.
    // trace:BUG-1918 | ai:claude
    fn policy(verdicts: &[RecordedVerdict]) -> ShipApprovalPolicy {
        let mut p = ShipApprovalPolicy {
            reviewed_committed_at: Some("2026-09-22T00:00:00Z".parse().unwrap()),
            // An independent operator runs the merge.
            shipper: vec!["session:independent-shipper".into()],
            ..Default::default()
        };
        for v in verdicts {
            if let Some(a) = v.attestation.as_ref() {
                if let Some(k) = a.authority_key(v.recorded_at.as_deref()) {
                    p.verified_authority.insert(k, a.identity.clone());
                }
            }
        }
        p
    }

    fn activity(root: &std::path::Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(root.join(".aida").join("advisor-activity.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn covering_approval_pins_the_merge_to_the_approved_head() {
        let tmp = tempfile::tempdir().unwrap();
        let pin = pr_ship_approval_gate(
            tmp.path(),
            5,
            &[approval(HEAD)],
            Some(HEAD),
            &policy(&[approval(HEAD)]),
            false,
            true,
            None,
        )
        .expect("an approval at the head merges");
        assert_eq!(pin.as_deref(), Some(HEAD));
        assert!(activity(tmp.path()).is_empty(), "nothing to audit");
    }

    /// BUG-1918: no approval always refuses — there is no review opt-out —
    /// and `--override-stale-approval` never excuses it.
    // trace:BUG-1918 | ai:claude
    #[test]
    fn no_approval_always_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let err = pr_ship_approval_gate(
            tmp.path(),
            5,
            &[],
            Some(HEAD),
            &policy(&[]),
            false,
            true,
            None,
        )
        .expect_err("no approval refuses");
        assert!(err.to_string().contains("no approval recorded"), "{err}");
        assert!(pr_ship_approval_gate(
            tmp.path(),
            5,
            &[],
            Some(HEAD),
            &policy(&[]),
            true,
            true,
            None
        )
        .is_err());
    }

    /// BUG-1918 (round-2 regression): an honest approval of an older commit,
    /// recorded BEFORE the current head was pushed, passes the authority gate
    /// and reaches the TASK-1458 head gate and its override — it is not
    /// mistaken for a backdated approval.
    // trace:BUG-1918 | ai:claude
    #[test]
    fn honest_stale_approval_older_than_the_head_reaches_the_override() {
        let approved_at: chrono::DateTime<chrono::Utc> =
            approval(OLD).recorded_at.unwrap().parse().unwrap();
        let head_at: chrono::DateTime<chrono::Utc> = HEAD_COMMITTED_AT.parse().unwrap();
        assert!(
            head_at > approved_at,
            "the head was pushed after the review"
        );
        let p = policy(&[approval(OLD)]);
        assert_eq!(
            crate::pr_ship::approval_authority_refusal(&[approval(OLD)], &p),
            None,
            "authority holds: the approval post-dates the commit it reviewed"
        );
        let tmp = tempfile::tempdir().unwrap();
        let err = pr_ship_approval_gate(
            tmp.path(),
            5,
            &[approval(OLD)],
            Some(HEAD),
            &p,
            false,
            true,
            None,
        )
        .expect_err("stale without the override");
        assert!(
            err.to_string().contains("--override-stale-approval"),
            "{err}"
        );
        assert!(
            pr_ship_approval_gate(
                tmp.path(),
                5,
                &[approval(OLD)],
                Some(HEAD),
                &p,
                true,
                true,
                None
            )
            .is_ok(),
            "the override applies"
        );
    }

    /// BUG-1918 (round 3): the author cannot ship its own unreviewed commits
    /// on top of an older independent approval via `--override-stale-approval`,
    /// and the refusal does not advertise the override to it. An unanchored
    /// shipper is refused the override too.
    // trace:BUG-1918 | ai:claude
    #[test]
    fn stale_approval_override_is_refused_to_the_author() {
        let tmp = tempfile::tempdir().unwrap();
        let mut by_author = policy(&[approval(OLD)]);
        by_author.author_sessions = vec![crate::pr_ship::AuthorSession {
            label: "lease aaaabbbbcccc".into(),
            tokens: vec!["session:the-author".into()],
        }];
        by_author.shipper = vec!["session:the-author".into(), "proc:42@t".into()];
        assert_eq!(
            crate::pr_ship::approval_authority_refusal(&[approval(OLD)], &by_author),
            None,
            "the older approval itself is independent"
        );
        for override_stale in [false, true] {
            let err = pr_ship_approval_gate(
                tmp.path(),
                5,
                &[approval(OLD)],
                Some(HEAD),
                &by_author,
                override_stale,
                true,
                None,
            )
            .expect_err("the author never ships unreviewed commits");
            let msg = err.to_string();
            assert!(
                !msg.contains("re-run with `--override-stale-approval`"),
                "the override is not advertised to the author: {msg}"
            );
            if override_stale {
                assert!(msg.contains("authored the work"), "{msg}");
            }
        }
        let mut unanchored = policy(&[approval(OLD)]);
        unanchored.shipper = vec!["lease:aaaabbbbcccc".into()];
        let err = pr_ship_approval_gate(
            tmp.path(),
            5,
            &[approval(OLD)],
            Some(HEAD),
            &unanchored,
            true,
            true,
            None,
        )
        .expect_err("an unanchored shipper cannot override");
        assert!(err.to_string().contains("no identity"), "{err}");
        // Nothing was overridden: no override record.
        assert!(activity(tmp.path())
            .iter()
            .all(|e| e["step"] != "pr-merge-override-stale-approval"));
    }

    #[test]
    fn stale_approval_refuses_and_logs_a_failed_merge_step() {
        let tmp = tempfile::tempdir().unwrap();
        let err = pr_ship_approval_gate(
            tmp.path(),
            5,
            &[approval(OLD)],
            Some(HEAD),
            &policy(&[approval(OLD)]),
            false,
            true,
            None,
        )
        .expect_err("a stale approval refuses");
        assert!(
            err.to_string().contains("--override-stale-approval"),
            "{err}"
        );
        let log = activity(tmp.path());
        assert_eq!(log.len(), 1);
        assert_eq!(log[0]["step"], "pr-merge");
        assert_eq!(log[0]["status"], "failed");
    }

    // STORY-1436: the held gate leaves a counted record; an override does not.
    // trace:STORY-1436 | ai:claude
    #[test]
    fn stale_approval_refusal_records_a_gate_held_event() {
        let tmp = tempfile::tempdir().unwrap();
        let _on = crate::test_env::EnvVarGuard::unset(crate::events::EVENTS_DISABLE_ENV);
        let _ = pr_ship_approval_gate(
            tmp.path(),
            5,
            &[approval(OLD)],
            Some(HEAD),
            &policy(&[approval(OLD)]),
            false,
            true,
            None,
        );
        let evs = crate::events::read_all(tmp.path());
        assert_eq!(evs.len(), 1, "{evs:?}");
        assert!(matches!(
            &evs[0].kind,
            crate::events::EventKind::GateHeld { gate, pr: Some(5), .. }
                if gate == crate::events::GATE_STALE_APPROVAL
        ));
        let other = tempfile::tempdir().unwrap();
        let _ = pr_ship_approval_gate(
            other.path(),
            5,
            &[approval(OLD)],
            Some(HEAD),
            &policy(&[approval(OLD)]),
            true,
            true,
            None,
        );
        assert!(crate::events::read_all(other.path()).is_empty());
    }

    #[test]
    fn override_is_recorded_durably_with_both_shas_and_pins_the_overridden_head() {
        let tmp = tempfile::tempdir().unwrap();
        let pin = pr_ship_approval_gate(
            tmp.path(),
            5,
            &[approval(OLD)],
            Some(HEAD),
            &policy(&[approval(OLD)]),
            true,
            true,
            None,
        )
        .expect("the override ships");
        assert_eq!(
            pin.as_deref(),
            Some(HEAD),
            "the override is granted for THIS head only"
        );
        let log = activity(tmp.path());
        assert_eq!(log.len(), 1, "{log:?}");
        assert_eq!(log[0]["step"], "pr-merge-override-stale-approval");
        assert_eq!(log[0]["pr"], 5);
        assert_eq!(log[0]["reviewed_sha"], OLD);
        assert_eq!(log[0]["head_sha"], HEAD);
    }
}

/// TASK-1458: source-shape guard — `pr_ship_handler` routes its merge
/// through `pr_ship_approval_gate` and hands the returned pin to the forge
/// merge, so the tested gate is the one ship runs. Needles are split so this
/// block cannot match its own literals.
// trace:TASK-1458 | ai:claude
#[cfg(test)]
mod task_1458_pr_ship_wiring_guard {
    #[test]
    fn pr_ship_handler_pins_its_merge_through_the_approval_gate() {
        let src = include_str!("pr_cmd.rs");
        let start = src
            .find(concat!("pub(crate) fn pr_ship_", "handler("))
            .expect("handler");
        let body = &src[start..];
        let gate = body
            .find(concat!("let match_head = pr_ship_", "approval_gate("))
            .expect("the handler runs the approval gate");
        let opts = body
            .find(concat!("match_head, // trace:", "TASK-1458"))
            .expect("the gate's pin reaches MergeOptions");
        let merge = body
            .find(concat!(".merge_", "change("))
            .expect("the handler merges");
        assert!(gate < opts && opts < merge, "gate → pin → merge order");
    }
}

// BUG-2217: `aida ship --no-merge` opens the PR and must file its reviewer
// story — PR-2895 stalled with "no queued review story" because the StopAtPr
// arm never queued one. trace:BUG-2217 | ai:claude
#[cfg(test)]
mod bug_2217_ship_no_merge_files_review_story {
    #[test]
    fn stop_at_pr_queues_the_review_story() {
        let src = include_str!("pr_cmd.rs");
        let arm = src
            .find(concat!("FinishMode::StopAt", "Pr => {"))
            .expect("the --no-merge arm");
        let body = &src[arm..];
        let queue = body
            .find(concat!("queue_review_for_", "open_pr("))
            .expect("the StopAtPr arm files the reviewer story");
        let banner = body
            .find(concat!("--no-merge; not ", "merged"))
            .expect("the pause banner");
        assert!(queue < banner, "the story is filed before the pause banner");
    }

    #[test]
    fn every_skip_prints_the_rerun_command() {
        let src = include_str!("pr_cmd.rs");
        let start = src
            .find(concat!("pub(crate) fn queue_review_for_", "open_pr("))
            .expect("the shared step");
        let end = src[start..].find("\n}\n").map(|e| start + e).unwrap();
        let body = &src[start..end];
        assert_eq!(
            body.matches(concat!("aida pr auto-queue-review --", "branch"))
                .count(),
            2,
            "both skip arms print the re-run command"
        );
    }
}

/// Pull-request side-effects. Today: `auto-queue-review` files the
/// reviewer story right after a change opens, so the trigger lives
/// where context is freshest. `aida session end` keeps the same logic
/// as an idempotent backup for changes opened without it.
// trace:STORY-90 trace:TASK-2251 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "pr")]
pub(crate) struct PrArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::PrCommand,
}

// STORY-90: PR side-effects dispatch before storage init — the
// command's side-effects all live in git / gh / self-invoked `aida
// add` calls; it doesn't need the global Storage handle.
// trace:STORY-90 | ai:claude
fn run_pr(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <PrArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let pr_cmd = &args.inner;
    return handle_pr_command(pr_cmd);
}

// trace:STORY-1728 | ai:claude
pub(crate) static PR: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "pr",
    help_after: "ship",
    command: <PrArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Early(run_pr),
};

/// The one-shot HUMAN-implementer finish. From inside a worktree where
/// you implemented a spec, run the whole finish ceremony in ONE command:
/// commit any uncommitted work with the (SPEC-ID) trailer, rebase onto
/// current origin/main, push, open the PR, wait for CI green, squash-merge,
/// run `aida pull` (Done → Completed auto-bump), and remove the worktree.
///
/// It is the orchestrator's finish phases with phase-1 (implement) done by
/// you instead of a spawned agent — the human-implementer counterpart to
/// `aida queue work <id> --auto-complete`. The spec is resolved from the
/// current branch (or the active session lease); pass `<spec>` to override.
///
/// Stop earlier with `--no-merge` (open the PR, don't merge) or `--no-pr`
/// (just rebase + push). `--keep-worktree` leaves the worktree in place
/// after a successful merge.
#[derive(clap::Parser, Debug)]
#[clap(name = "ship")]
pub(crate) struct ShipArgs {
    /// Spec to finish. When omitted, resolved from the current branch
    /// name (e.g. `<spec>-ship` → `<SPEC-ID>`) or the active session
    /// lease covering this worktree.
    #[clap(value_name = "SPEC")]
    pub spec: Option<String>,

    /// Commit subject to use when committing uncommitted work. The
    /// `(SPEC-ID)` trailer is appended automatically. When omitted, a
    /// conventional default is used. Ignored when the worktree is clean.
    // trace:BUG-1294 | ai:claude
    #[clap(long, short = 'm', value_name = "MSG", allow_hyphen_values = true)]
    pub message: Option<String>,

    /// Stop after opening the PR — do not watch CI or merge. The PR is
    /// left open for a human to review + merge.
    #[clap(long)]
    pub no_merge: bool,

    /// Stop after rebase + push — do not open a PR. Useful when you want
    /// to open the PR yourself, or when CI is configured to gate on push.
    #[clap(long, conflicts_with = "no_merge")]
    pub no_pr: bool,

    /// Keep the worktree after a successful merge (skip the
    /// `aida session end` cleanup step).
    #[clap(long)]
    pub keep_worktree: bool,

    /// Print the resolved finish sequence without executing any of it.
    #[clap(long)]
    pub dry_run: bool,

    /// Bypass the client-side trailer spec-ID check before shipping.
    // trace:STORY-469 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(long)]
    pub no_trailer_check: bool,
}

// STORY-720: `aida ship` dispatches before storage init for the same
// reason as `aida pr` — every side-effect is a git / gh / self-invoked
// `aida` call (the finish tail reuses `pr_ship_handler`). The spec is
// resolved from the branch / lease, not the requirement store.
// trace:STORY-720 | ai:claude
fn run_ship(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <ShipArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let ShipArgs {
        spec,
        message,
        no_merge,
        no_pr,
        keep_worktree,
        dry_run,
        no_trailer_check,
    } = &args;
    return run_human_finish_ceremony(HumanFinishOptions {
        spec: spec.clone(),
        message: message.clone(),
        no_merge: *no_merge,
        no_pr: *no_pr,
        keep_worktree: *keep_worktree,
        dry_run: *dry_run,
        no_trailer_check: *no_trailer_check,
    });
}

// trace:STORY-1728 | ai:claude
pub(crate) static SHIP: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "ship",
    help_after: "triage",
    command: <ShipArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::BuildLoop,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Early(run_ship),
};

#[cfg(test)]
mod story_1663_rebase_ship_tests {
    use super::*;
    use clap::Parser;

    // trace:TASK-1951 | ai:claude
    fn task_1951_repo() -> (tempfile::TempDir, impl Fn(&[&str]) -> String) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let git = move |args: &[&str]| {
            let out = crate::test_git::git_command()
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git {args:?} failed");
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        std::fs::write(tmp.path().join("README.md"), "init\n").unwrap();
        git(&["add", "README.md"]);
        git(&["commit", "-q", "-m", "chore: initial"]);
        (tmp, git)
    }

    // trace:TASK-1951 trace:TASK-1619 | ai:claude
    fn commit_stub_verifier(root: &std::path::Path, git: &dyn Fn(&[&str]) -> String, body: &str) {
        let dir = root.join("scripts/split-lib");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("verify_stack.py"), body).unwrap();
        git(&["add", "scripts/split-lib/verify_stack.py"]);
        git(&["commit", "-q", "-m", "chore: verifier"]);
    }

    /// TASK-1619: the verifier is read from the base the PR merges into, not
    /// from origin/main, and a passing verifier lets the ship proceed. A base
    /// that resolves without the script never falls back to the local copy.
    // trace:TASK-1951 trace:TASK-1619 | ai:claude
    #[test]
    fn task_1951_slice_verifier_reads_the_merge_base_and_passes() {
        let (tmp, git) = task_1951_repo();
        let root = tmp.path();
        let ran = root.join("release-verifier-ran");
        commit_stub_verifier(
            root,
            &git,
            "import sys\nprint('FAIL main verifier')\nsys.exit(1)\n",
        );
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        commit_stub_verifier(
            root,
            &git,
            &format!(
                "import sys\nopen({:?}, 'w').write(' '.join(sys.argv[1:]))\nprint('PASS')\n",
                ran.display().to_string()
            ),
        );
        git(&["update-ref", "refs/remotes/origin/release", "HEAD"]);
        std::fs::write(root.join("x.txt"), "moved\n").unwrap();
        git(&["add", "x.txt"]);
        git(&["commit", "-q", "-m", "refactor(cli-lib): extract foo.rs"]);

        run_optional_slice_stack_verifier(root, root, 7, "origin/release", "HEAD")
            .expect("the release base's passing verifier admits the slice");
        assert_eq!(
            std::fs::read_to_string(&ran).unwrap(),
            "origin/release HEAD"
        );

        // A resolvable base without the script: no verifier, even though the
        // local checkout has a failing one.
        let bare = git(&["rev-parse", "HEAD~3"]);
        git(&["update-ref", "refs/remotes/origin/bare", &bare]);
        run_optional_slice_stack_verifier(root, root, 7, "origin/bare", "HEAD")
            .expect("a base without verify_stack.py has no verifier");
    }

    /// TASK-1620 round 2: two slice commits sharing a subject both land
    /// under their own shas and both are marked. TASK-1622: an unreadable
    /// range is an error even when no spec id was found.
    // trace:TASK-1951 trace:TASK-1620 trace:TASK-1622 | ai:claude
    #[test]
    fn task_1951_duplicate_subject_slices_are_each_marked_and_unreadable_ranges_fail() {
        let (tmp, git) = task_1951_repo();
        let root = tmp.path();
        let store_dir = root.join(".aida-store");
        let backend = crate::test_env::home_fixture_backend(&store_dir).unwrap();
        use aida_core::db::DatabaseBackend;
        let mut req = aida_core::Requirement::new("Slice parent".into(), String::new());
        req.spec_id = Some("STORY-78".to_string());
        req.status = aida_core::RequirementStatus::InProgress;
        backend.add_requirement(req).unwrap();

        git(&["switch", "-q", "-c", "claude/story-78-part1"]);
        for n in ["one", "two"] {
            std::fs::write(root.join(format!("{n}.txt")), "x\n").unwrap();
            git(&["add", &format!("{n}.txt")]);
            git(&["commit", "-q", "-m", "refactor: move code (STORY-78)"]);
        }
        let pr_head = git(&["rev-parse", "HEAD"]);
        git(&["switch", "-q", "main"]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&["cherry-pick", "main..claude/story-78-part1"]);
        let landed = [git(&["rev-parse", "HEAD"]), git(&["rev-parse", "HEAD~1"])];

        record_non_completing_rebase_slice(
            root,
            root,
            78,
            "main",
            Some(&pr_head),
            "claude/story-78-part1",
            &[],
        )
        .expect("marker recorded");
        let stored = backend
            .get_requirement_unambiguous("STORY-78")
            .unwrap()
            .unwrap();
        for sha in &landed {
            assert!(
                crate::db_reconcile::spec_ignores_non_completing_rebase_commit(root, &stored, sha),
                "landed {sha} is marked"
            );
        }

        let err = record_non_completing_rebase_slice(
            root,
            root,
            79,
            "main",
            Some("0123456789abcdef0123456789abcdef01234567"),
            "no-spec-branch",
            &[],
        )
        .expect_err("an unreadable range fails before the empty-candidates return");
        assert!(err.to_string().contains("commit subjects"), "{err:#}");
    }

    /// TASK-1620: the marker names the commit as it landed on the base after
    /// the rebase rewrote it, and a later commit reusing the subject is not
    /// suppressed. TASK-1622: an unopenable store is an error, not silence.
    // trace:TASK-1951 trace:TASK-1620 trace:TASK-1622 | ai:claude
    #[test]
    fn task_1951_non_completing_marker_matches_landed_shas_only_and_reports_failures() {
        let (tmp, git) = task_1951_repo();
        let root = tmp.path();
        let store_dir = root.join(".aida-store");
        let backend = crate::test_env::home_fixture_backend(&store_dir).unwrap();
        use aida_core::db::DatabaseBackend;
        let mut req = aida_core::Requirement::new("Slice parent".into(), String::new());
        req.spec_id = Some("STORY-77".to_string());
        req.status = aida_core::RequirementStatus::InProgress;
        backend.add_requirement(req).unwrap();

        git(&["switch", "-q", "-c", "claude/story-77-part1"]);
        std::fs::write(root.join("slice.txt"), "one\n").unwrap();
        git(&["add", "slice.txt"]);
        git(&["commit", "-q", "-m", "feat: slice one (STORY-77)"]);
        let pr_head = git(&["rev-parse", "HEAD"]);
        // The rebase merge: main moved on, then the slice lands rewritten.
        git(&["switch", "-q", "main"]);
        std::fs::write(root.join("other.txt"), "other\n").unwrap();
        git(&["add", "other.txt"]);
        git(&["commit", "-q", "-m", "chore: unrelated"]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
        git(&["cherry-pick", &pr_head]);
        let landed = git(&["rev-parse", "HEAD"]);
        assert_ne!(landed, pr_head);

        record_non_completing_rebase_slice(
            root,
            root,
            77,
            "main",
            Some(&pr_head),
            "claude/story-77-part1",
            &[],
        )
        .expect("marker recorded");
        let stored = || {
            backend
                .get_requirement_unambiguous("STORY-77")
                .unwrap()
                .unwrap()
        };
        let ignores = |sha: &str| {
            crate::db_reconcile::spec_ignores_non_completing_rebase_commit(root, &stored(), sha)
        };
        assert!(ignores(&landed), "the landed commit is the slice");

        std::fs::write(root.join("slice.txt"), "re-land\n").unwrap();
        git(&["add", "slice.txt"]);
        git(&["commit", "-q", "-m", "feat: slice one (STORY-77)"]);
        let relanded = git(&["rev-parse", "HEAD"]);
        assert!(
            !ignores(&relanded),
            "a later commit reusing the subject is not the recorded slice"
        );

        let no_store = tempfile::tempdir().unwrap();
        let err = record_non_completing_rebase_slice(
            no_store.path(),
            root,
            77,
            "main",
            Some(&pr_head),
            "claude/story-77-part1",
            &[],
        )
        .expect_err("a missing store must be reported");
        assert!(err.to_string().contains("store"), "{err:#}");
    }

    fn task_1951_merged_meta(
        state: crate::forge::ChangeState,
        branch: &str,
        sha: &str,
    ) -> crate::forge::ChangeMetadata {
        crate::forge::ChangeMetadata {
            state,
            title: "feat: slice one".into(),
            author: None,
            merged_at: None,
            base_ref: "main".into(),
            head_ref: branch.into(),
            head_sha: sha.into(),
            is_draft: false,
            is_cross_repository: false,
            head_repo: None,
        }
    }

    /// TASK-1621: an already-merged re-run, from a `main` checkout whose tip
    /// names another spec, resolves the merged PR's own branch, base, head
    /// and commits and marks only the PR's spec, once, however often it
    /// re-runs. When the merged PR cannot be resolved it refuses and names
    /// the `--rebase` re-run, and the pull is skipped (TASK-1622, deferral
    /// (d) of #2816).
    // trace:TASK-1951 trace:TASK-1621 trace:TASK-1622 | ai:claude
    #[test]
    fn task_1951_already_merged_rerun_marks_only_the_merged_prs_spec() {
        assert!(rebase_slice_bookkeeping_due(false, true));
        assert!(!rebase_slice_bookkeeping_due(false, false));

        let (tmp, git) = task_1951_repo();
        let root = tmp.path();
        let store_dir = root.join(".aida-store");
        let backend = crate::test_env::home_fixture_backend(&store_dir).unwrap();
        use aida_core::db::DatabaseBackend;
        for (id, status) in [
            ("STORY-77", aida_core::RequirementStatus::InProgress),
            ("STORY-88", aida_core::RequirementStatus::Done),
        ] {
            let mut req = aida_core::Requirement::new(format!("{id} spec"), String::new());
            req.spec_id = Some(id.to_string());
            req.status = status;
            backend.add_requirement(req).unwrap();
        }
        git(&["switch", "-q", "-c", "claude/story-77-part1"]);
        std::fs::write(root.join("slice.txt"), "one\n").unwrap();
        git(&["add", "slice.txt"]);
        git(&["commit", "-q", "-m", "feat: slice one (STORY-77)"]);
        let pr_head = git(&["rev-parse", "HEAD"]);
        git(&["switch", "-q", "main"]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&["cherry-pick", &pr_head]);
        let landed = git(&["rev-parse", "HEAD"]);
        std::fs::write(root.join("other.txt"), "other\n").unwrap();
        git(&["add", "other.txt"]);
        git(&["commit", "-q", "-m", "fix: other work (STORY-88)"]);
        // Deferral (b): this clone never fetched the PR head object.
        git(&["branch", "-D", "claude/story-77-part1"]);
        git(&["reflog", "expire", "--expire=now", "--all"]);
        git(&["gc", "-q", "--prune=now"]);

        let meta = task_1951_merged_meta(
            crate::forge::ChangeState::Merged,
            "claude/story-77-part1",
            &pr_head,
        );
        let run = RebaseSliceRun {
            main_worktree: root,
            project_root: root,
            pr_number: 77,
            final_slice: false,
            already_merged: true,
            // The checkout is `main`, as when the advisor re-runs ship.
            ship_branch: "main",
            retarget_base: "main",
            merged_head_sha: None,
        };
        for _ in 0..2 {
            let outcome = rebase_slice_bookkeeping(
                &run,
                &[],
                |_| unreachable!("no final slice"),
                || {
                    merged_pr_refs_from_metadata(
                        &meta,
                        vec!["feat: slice one (STORY-77)".to_string()],
                    )
                },
            );
            assert_eq!(outcome, SliceBookkeeping::default());
        }
        let stored = |id: &str| backend.get_requirement_unambiguous(id).unwrap().unwrap();
        let markers = |id: &str| {
            stored(id)
                .comments
                .iter()
                .filter(|c| c.content.contains(pr_ship::NON_COMPLETING_REBASE_MARKER))
                .count()
        };
        assert_eq!(markers("STORY-77"), 1, "one marker for the PR's spec");
        assert_eq!(markers("STORY-88"), 0, "main's tip spec is untouched");
        assert!(
            crate::db_reconcile::spec_ignores_non_completing_rebase_commit(
                root,
                &stored("STORY-77"),
                &landed
            )
        );

        let unresolved = rebase_slice_bookkeeping(
            &RebaseSliceRun {
                pr_number: 78,
                ..run
            },
            &[],
            |_| unreachable!(),
            || anyhow::bail!("forge unavailable"),
        );
        let warning = unresolved.warning.unwrap();
        assert!(
            warning.contains("PR-78") && warning.contains("forge unavailable"),
            "{warning}"
        );
        assert!(
            warning.contains("re-run `aida pr ship 78 --rebase`"),
            "{warning}"
        );
        assert_eq!(
            unresolved.pull_skip,
            Some("non-completing slice marker not recorded")
        );
        assert_eq!(post_merge_pull_skip_reason(false), None);

        let sha = "0123456789abcdef0123456789abcdef01234567";
        for bad in [
            task_1951_merged_meta(crate::forge::ChangeState::Open, "b", sha),
            task_1951_merged_meta(crate::forge::ChangeState::Merged, "b", ""),
            task_1951_merged_meta(crate::forge::ChangeState::Merged, "-x", sha),
        ] {
            assert!(
                merged_pr_refs_from_metadata(&bad, Vec::new()).is_err(),
                "{bad:?}"
            );
        }
    }

    /// TASK-2130: `--rebase` on a re-run of a PR that was squash-merged
    /// records nothing and completes nothing, even with `--final-slice`.
    /// TASK-1621 (a): a final slice that was rebase-merged completes the
    /// specs of the merged PR's branch, not the checkout's.
    // trace:TASK-1951 trace:TASK-2130 trace:TASK-1621 | ai:claude
    #[test]
    fn task_1951_already_merged_rerun_confirms_the_rebase_merge() {
        let (tmp, git) = task_1951_repo();
        let root = tmp.path();
        let backend = crate::test_env::home_fixture_backend(&root.join(".aida-store")).unwrap();
        use aida_core::db::DatabaseBackend;
        for id in ["STORY-91", "STORY-92"] {
            let mut req = aida_core::Requirement::new(format!("{id} spec"), String::new());
            req.spec_id = Some(id.to_string());
            req.status = aida_core::RequirementStatus::InProgress;
            backend.add_requirement(req).unwrap();
        }
        // STORY-91's two commits were squash-merged as one.
        git(&["switch", "-q", "-c", "claude/story-91-part1"]);
        for n in ["a", "b"] {
            std::fs::write(root.join(n), "x\n").unwrap();
            git(&["add", n]);
            git(&["commit", "-q", "-m", &format!("feat: part {n} (STORY-91)")]);
        }
        let squashed_head = git(&["rev-parse", "HEAD"]);
        git(&["switch", "-q", "main"]);
        git(&["merge", "-q", "--squash", "claude/story-91-part1"]);
        git(&["commit", "-q", "-m", "feat: slice (#91)"]);
        // STORY-92 was rebase-merged.
        git(&["switch", "-q", "-c", "claude/story-92-part2"]);
        std::fs::write(root.join("c"), "x\n").unwrap();
        git(&["add", "c"]);
        git(&["commit", "-q", "-m", "feat: last part (STORY-92)"]);
        let rebased_head = git(&["rev-parse", "HEAD"]);
        git(&["switch", "-q", "main"]);
        git(&["cherry-pick", &rebased_head]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);

        let run = |pr, final_slice| RebaseSliceRun {
            main_worktree: root,
            project_root: root,
            pr_number: pr,
            final_slice,
            already_merged: true,
            ship_branch: "main",
            retarget_base: "main",
            merged_head_sha: None,
        };
        for final_slice in [false, true] {
            let outcome = rebase_slice_bookkeeping(
                &run(91, final_slice),
                &[],
                |_| panic!("a squash merge must complete nothing"),
                || {
                    merged_pr_refs_from_metadata(
                        &task_1951_merged_meta(
                            crate::forge::ChangeState::Merged,
                            "claude/story-91-part1",
                            &squashed_head,
                        ),
                        vec![
                            "feat: part a (STORY-91)".to_string(),
                            "feat: part b (STORY-91)".to_string(),
                        ],
                    )
                },
            );
            let warning = outcome.warning.unwrap();
            assert!(warning.contains("was not rebase-merged"), "{warning}");
        }
        let story = |id: &str| backend.get_requirement_unambiguous(id).unwrap().unwrap();
        assert!(story("STORY-91").comments.is_empty());
        assert_eq!(
            story("STORY-91").status,
            aida_core::RequirementStatus::InProgress
        );

        let asked = std::cell::RefCell::new(None);
        let outcome = rebase_slice_bookkeeping(
            &run(92, true),
            &["STORY-91".to_string()],
            |branch| {
                *asked.borrow_mut() = Some(branch.to_string());
                vec!["STORY-92".to_string()]
            },
            || {
                merged_pr_refs_from_metadata(
                    &task_1951_merged_meta(
                        crate::forge::ChangeState::Merged,
                        "claude/story-92-part2",
                        &rebased_head,
                    ),
                    vec!["feat: last part (STORY-92)".to_string()],
                )
            },
        );
        assert_eq!(outcome, SliceBookkeeping::default());
        assert_eq!(asked.borrow().as_deref(), Some("claude/story-92-part2"));
        assert_eq!(
            story("STORY-92").status,
            aida_core::RequirementStatus::Completed
        );
        assert_eq!(
            story("STORY-91").status,
            aida_core::RequirementStatus::InProgress
        );
    }

    /// TASK-2132 + #2816 deferral (c): the landed commits are matched as one
    /// consecutive run, so a newer commit reusing a slice subject is not
    /// bound in place of the real one, and a stale `origin/<base>` that does
    /// not hold the run yet matches nothing there.
    // trace:TASK-1951 trace:TASK-2132 | ai:claude
    #[test]
    fn task_1951_landed_run_ignores_later_subject_reuse_and_stale_refs() {
        let log = [
            ("f", "feat: slice one (STORY-5)"),
            ("e", "chore: unrelated"),
            ("d", "feat: slice two (STORY-5)"),
            ("c", "feat: slice one (STORY-5)"),
            ("b", "chore: base"),
        ];
        let subjects = vec![
            "feat: slice two (STORY-5)".to_string(),
            "feat: slice one (STORY-5)".to_string(),
        ];
        assert_eq!(
            landed_run(&log, &subjects, false),
            Some(vec!["d".to_string(), "c".to_string()])
        );
        let oldest_first: Vec<String> = subjects.iter().rev().cloned().collect();
        assert_eq!(
            landed_run(&log, &oldest_first, false),
            Some(vec!["c".to_string(), "d".to_string()])
        );
        assert_eq!(
            landed_run(&log[..2], &subjects, false),
            None,
            "a stale ref holds no run"
        );

        let (tmp, git) = task_1951_repo();
        let root = tmp.path();
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        std::fs::write(root.join("s"), "x\n").unwrap();
        git(&["add", "s"]);
        git(&["commit", "-q", "-m", "feat: slice one (STORY-5)"]);
        let real = git(&["rev-parse", "HEAD"]);
        std::fs::write(root.join("t"), "x\n").unwrap();
        git(&["add", "t"]);
        git(&["commit", "-q", "-m", "chore: unrelated"]);
        // origin/main is stale (never fetched past the base); the local base
        // holds the run.
        assert_eq!(
            landed_commits_for_subjects(
                root,
                "main",
                None,
                &["feat: slice one (STORY-5)".to_string()]
            ),
            vec![Some(real)]
        );
    }

    /// TASK-2132, review round 1: with the PR head known, the walk stops at
    /// the PR's fork point and takes the oldest run above it, so a later
    /// commit reusing the subject of a one-commit slice (n = 1), or a whole
    /// run repeated later, never binds in place of the slice.
    // trace:TASK-1951 trace:TASK-2132 | ai:claude
    #[test]
    fn task_1951_landed_run_is_bounded_at_the_pr_fork_point() {
        let one = vec!["feat: slice one (STORY-5)".to_string()];
        let log = [
            ("r", "feat: slice one (STORY-5)"),
            ("x", "chore: unrelated"),
            ("s", "feat: slice one (STORY-5)"),
        ];
        assert_eq!(landed_run(&log, &one, true), Some(vec!["s".to_string()]));
        assert_eq!(landed_run(&log, &one, false), Some(vec!["r".to_string()]));
        let two = vec![
            "feat: slice two (STORY-5)".to_string(),
            "feat: slice one (STORY-5)".to_string(),
        ];
        let repeated = [
            ("d2", "feat: slice two (STORY-5)"),
            ("c2", "feat: slice one (STORY-5)"),
            ("x", "chore: unrelated"),
            ("d", "feat: slice two (STORY-5)"),
            ("c", "feat: slice one (STORY-5)"),
        ];
        assert_eq!(
            landed_run(&repeated, &two, true),
            Some(vec!["d".to_string(), "c".to_string()])
        );

        let (tmp, git) = task_1951_repo();
        let root = tmp.path();
        git(&["switch", "-q", "-c", "claude/story-5-part1"]);
        std::fs::write(root.join("s"), "x\n").unwrap();
        git(&["add", "s"]);
        git(&["commit", "-q", "-m", "feat: slice one (STORY-5)"]);
        let pr_head = git(&["rev-parse", "HEAD"]);
        git(&["switch", "-q", "main"]);
        std::fs::write(root.join("m"), "x\n").unwrap();
        git(&["add", "m"]);
        git(&["commit", "-q", "-m", "chore: base moved"]);
        git(&["cherry-pick", &pr_head]);
        let landed = git(&["rev-parse", "HEAD"]);
        std::fs::write(root.join("t"), "x\n").unwrap();
        git(&["add", "t"]);
        git(&[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "feat: slice one (STORY-5)",
        ]);
        let reuse = git(&["rev-parse", "HEAD"]);
        assert_eq!(
            landed_commits_for_subjects(root, "main", Some(&pr_head), &one),
            vec![Some(landed)]
        );
        assert_eq!(
            landed_commits_for_subjects(root, "main", None, &one),
            vec![Some(reuse)],
            "without a head the newest run is taken, as before"
        );
    }

    /// TASK-2131, review round 1: a pure-git already-merged `--rebase` re-run
    /// reads the slice's commits from `<base>..<slice branch>`, confirms the
    /// rebase merge on the base, and completes the slice branch's specs; an
    /// empty range is refused, never confirmed.
    // trace:TASK-1951 trace:TASK-2131 | ai:claude
    #[test]
    fn task_1951_pure_git_already_merged_rerun_uses_the_slice_branch() {
        let (tmp, git) = task_1951_repo();
        let root = tmp.path();
        let backend = crate::test_env::home_fixture_backend(&root.join(".aida-store")).unwrap();
        use aida_core::db::DatabaseBackend;
        let mut req = aida_core::Requirement::new("STORY-93 spec".to_string(), String::new());
        req.spec_id = Some("STORY-93".to_string());
        req.status = aida_core::RequirementStatus::InProgress;
        backend.add_requirement(req).unwrap();
        git(&["switch", "-q", "-c", "claude/story-93-part2"]);
        std::fs::write(root.join("c"), "x\n").unwrap();
        git(&["add", "c"]);
        git(&["commit", "-q", "-m", "feat: last part (STORY-93)"]);
        let slice_head = git(&["rev-parse", "HEAD"]);
        git(&["switch", "-q", "main"]);
        std::fs::write(root.join("m"), "x\n").unwrap();
        git(&["add", "m"]);
        git(&["commit", "-q", "-m", "chore: base moved"]);
        git(&["cherry-pick", &slice_head]);
        assert_eq!(
            crate::forge::resolve_forge_kind(root),
            crate::forge::ForgeKind::None
        );

        let inputs = fetch_merged_pr_refs(root, 93, "claude/story-93-part2", "main").unwrap();
        assert_eq!(
            inputs.subjects,
            vec!["feat: last part (STORY-93)".to_string()]
        );
        assert_eq!(inputs.head_sha.as_deref(), Some(slice_head.as_str()));

        let run = RebaseSliceRun {
            main_worktree: root,
            project_root: root,
            pr_number: 93,
            final_slice: true,
            already_merged: true,
            ship_branch: "claude/story-93-part2",
            retarget_base: "main",
            merged_head_sha: None,
        };
        let outcome = rebase_slice_bookkeeping(
            &run,
            &[],
            |branch| {
                assert_eq!(branch, "claude/story-93-part2");
                vec!["STORY-93".to_string()]
            },
            || fetch_merged_pr_refs(root, 93, "claude/story-93-part2", "main"),
        );
        assert_eq!(outcome, SliceBookkeeping::default());
        assert_eq!(
            backend
                .get_requirement_unambiguous("STORY-93")
                .unwrap()
                .unwrap()
                .status,
            aida_core::RequirementStatus::Completed
        );

        // A slice branch already on the base has an empty range: refused.
        git(&["branch", "-f", "claude/story-93-part2", "main"]);
        let err = fetch_merged_pr_refs(root, 93, "claude/story-93-part2", "main").unwrap_err();
        assert!(format!("{err:#}").contains("holds no commits"), "{err:#}");
    }

    /// TASK-2251 (TASK-2248): a pure-git slice merged by fast-forward (the
    /// base had not moved) leaves `<base>..<slice>` empty; its commits are read
    /// from the fork point the reflogs record, and the merge is confirmed. A
    /// branch that never had commits is still refused.
    // trace:TASK-2251 | ai:claude
    #[test]
    fn task_2251_pure_git_already_merged_rerun_confirms_a_fast_forwarded_slice() {
        let (tmp, git) = task_1951_repo();
        let root = tmp.path();
        git(&["switch", "-q", "-c", "claude/story-94-part1"]);
        for (file, subject) in [("a", "feat: one (STORY-94)"), ("b", "feat: two (STORY-94)")] {
            std::fs::write(root.join(file), "x\n").unwrap();
            git(&["add", file]);
            git(&["commit", "-q", "-m", subject]);
        }
        let slice_head = git(&["rev-parse", "HEAD"]);
        git(&["switch", "-q", "main"]);
        git(&["merge", "-q", "--ff-only", "claude/story-94-part1"]);
        assert_eq!(git(&["rev-parse", "main"]), slice_head);

        let inputs = fetch_merged_pr_refs(root, 94, "claude/story-94-part1", "main").unwrap();
        assert_eq!(
            inputs.subjects,
            vec![
                "feat: two (STORY-94)".to_string(),
                "feat: one (STORY-94)".to_string()
            ]
        );
        confirm_rebase_merged(root, 94, &inputs).unwrap();

        // Created at the base tip and never committed to: nothing to confirm.
        git(&["branch", "claude/story-94-part2", "main"]);
        let err = fetch_merged_pr_refs(root, 94, "claude/story-94-part2", "main").unwrap_err();
        assert!(format!("{err:#}").contains("holds no commits"), "{err:#}");
    }

    /// TASK-2251 review round 1: a branch with no commits of its own, synced
    /// onto a base that others moved (fast-forward merge, pull, reset), is on
    /// the base like a fast-forwarded slice, but its reflog shows no authoring
    /// entry: refused, so the others' commits are never confirmed as the
    /// slice's. A slice rebased before the merge forks where it was replayed.
    // trace:TASK-2251 | ai:claude
    #[test]
    fn task_2251_fast_forward_confirmation_requires_the_branch_to_author_its_tip() {
        let (tmp, git) = task_1951_repo();
        let root = tmp.path();
        let others_commit = |file: &str, subject: &str| {
            git(&["switch", "-q", "main"]);
            std::fs::write(root.join(file), "x\n").unwrap();
            git(&["add", file]);
            git(&["commit", "-q", "-m", subject]);
        };
        for (n, sync) in [
            (1, &["merge", "-q", "--ff-only", "main"][..]),
            (2, &["pull", "-q", "--ff-only", ".", "main"][..]),
            (3, &["reset", "-q", "--hard", "main"][..]),
        ] {
            let branch = format!("claude/story-96-part{n}");
            git(&["branch", &branch, "main"]);
            others_commit(&format!("o{n}"), &format!("feat: someone else {n}"));
            git(&["switch", "-q", &branch]);
            git(sync);
            assert_eq!(git(&["rev-parse", "HEAD"]), git(&["rev-parse", "main"]));
            let err = fetch_merged_pr_refs(root, 96, &branch, "main").unwrap_err();
            assert!(
                format!("{err:#}").contains("holds no commits"),
                "{sync:?}: {err:#}"
            );
        }

        // Rebased onto a moved base, then fast-forwarded: only its own commit.
        git(&["switch", "-q", "-c", "claude/story-97-part1", "main"]);
        std::fs::write(root.join("r"), "x\n").unwrap();
        git(&["add", "r"]);
        git(&["commit", "-q", "-m", "feat: rebased slice (STORY-97)"]);
        others_commit("o4", "feat: someone else 4");
        git(&["switch", "-q", "claude/story-97-part1"]);
        git(&["rebase", "-q", "main"]);
        git(&["switch", "-q", "main"]);
        git(&["merge", "-q", "--ff-only", "claude/story-97-part1"]);
        let inputs = fetch_merged_pr_refs(root, 97, "claude/story-97-part1", "main").unwrap();
        assert_eq!(
            inputs.subjects,
            vec!["feat: rebased slice (STORY-97)".to_string()]
        );
        confirm_rebase_merged(root, 97, &inputs).unwrap();

        // A rebase that replayed nothing leaves the tip on `onto`: refused.
        let log = |entries: &[(&str, &str)]| -> Vec<(String, String)> {
            entries
                .iter()
                .map(|(sha, msg)| (sha.to_string(), msg.to_string()))
                .collect()
        };
        let b = "claude/x";
        assert_eq!(
            authored_slice_start(
                &log(&[
                    ("bbb", "rebase (finish): refs/heads/claude/x onto bbb"),
                    ("aaa", "branch: Created from main"),
                ]),
                b,
                "bbb"
            ),
            None
        );
        assert_eq!(
            authored_slice_start(
                &log(&[
                    ("ddd", "commit: two"),
                    ("ccc", "rebase (finish): refs/heads/claude/x onto bbb"),
                    ("aaa", "commit (amend): one"),
                    ("a00", "cherry-pick: zero"),
                    ("000", "branch: Created from main"),
                ]),
                b,
                "ddd"
            ),
            Some("bbb".to_string())
        );
        assert_eq!(
            authored_slice_start(
                &log(&[
                    ("ccc", "commit: two"),
                    ("bbb", "commit (merge): m"),
                    ("aaa", "branch: Created from main")
                ]),
                b,
                "ccc"
            ),
            None,
            "a merge commit is not on the allowlist"
        );
    }

    /// TASK-2251 (TASK-2249): a stale `origin/<base>` behind the slice's fork
    /// point would count base commits as slice subjects, and the confirmation
    /// would refuse. `origin/<base>` is fetched first, so the range holds only
    /// the slice's own commit.
    // trace:TASK-2251 | ai:claude
    #[test]
    fn task_2251_pure_git_already_merged_rerun_fetches_a_stale_origin_base() {
        let (tmp, git) = task_1951_repo();
        let root = tmp.path();
        let remote = tempfile::tempdir().unwrap();
        crate::test_git::git_command()
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(remote.path())
            .output()
            .unwrap();
        git(&[
            "remote",
            "add",
            "origin",
            &remote.path().display().to_string(),
        ]);
        let initial = git(&["rev-parse", "HEAD"]);
        std::fs::write(root.join("m"), "x\n").unwrap();
        git(&["add", "m"]);
        git(&["commit", "-q", "-m", "chore: base moved"]);
        git(&["switch", "-q", "-c", "claude/story-95-part1"]);
        std::fs::write(root.join("s"), "x\n").unwrap();
        git(&["add", "s"]);
        git(&["commit", "-q", "-m", "feat: the slice (STORY-95)"]);
        let slice_head = git(&["rev-parse", "HEAD"]);
        // The remote rebase-merges the slice onto its main.
        git(&["switch", "-q", "-c", "landing", "main"]);
        git(&["cherry-pick", &slice_head]);
        git(&["push", "-q", "origin", "landing:main"]);
        git(&["switch", "-q", "claude/story-95-part1"]);
        git(&["branch", "-D", "-q", "landing"]);
        // Both local views of the base predate the fork point.
        git(&["branch", "-f", "main", &initial]);
        git(&["update-ref", "refs/remotes/origin/main", &initial]);

        let inputs = fetch_merged_pr_refs(root, 95, "claude/story-95-part1", "main").unwrap();
        assert_eq!(
            inputs.subjects,
            vec!["feat: the slice (STORY-95)".to_string()]
        );
        confirm_rebase_merged(root, 95, &inputs).unwrap();
    }

    /// TASK-1765: a non-completing squash ship writes the declaration into
    /// the squash body, rebuilt from the commits when the identity gate left
    /// it to the forge.
    // trace:TASK-1951 trace:TASK-1765 | ai:claude
    #[test]
    fn task_1951_non_completing_squash_ship_writes_the_marker_into_the_body() {
        assert_eq!(non_completing_squash_body(""), "aida: no-complete");
        assert_eq!(
            non_completing_squash_body("* a\n\n* b\n"),
            "* a\n\n* b\n\naida: no-complete"
        );
        assert!(crate::db_reconcile::commit_declares_non_completing(
            &format!("fix: x (#9)\n\n{}", non_completing_squash_body("* a"))
        ));
        // TASK-2117: the body a --no-complete squash ship builds reaches the
        // forge's merge argv, whether the identity gate rewrote the body or
        // left it to the forge; an ordinary ship leaves it to the forge.
        // trace:TASK-1951 trace:TASK-2117 | ai:claude
        use crate::forge::MergeMethod;
        let squash = |declares, sanitized: Option<&str>| {
            ship_squash_body(
                MergeMethod::Squash,
                declares,
                sanitized.map(str::to_string),
                || Ok("* fix: one (Refs TASK-9)".to_string()),
            )
            .unwrap()
        };
        for body in [squash(true, None), squash(true, Some("* sanitized"))] {
            let opts = crate::forge::MergeOptions {
                method: MergeMethod::Squash,
                squash_subject: None,
                squash_body: body,
                delete_branch: true,
                match_head: None,
            };
            let argv = crate::forge::github_merge_argv(9, &opts);
            let at = argv
                .iter()
                .position(|a| a == "--body")
                .expect("--body passed");
            assert!(argv[at + 1].ends_with("\n\naida: no-complete"), "{argv:?}");
        }
        assert_eq!(
            squash(false, None),
            None,
            "an ordinary ship leaves the body to the forge"
        );
        assert_eq!(
            ship_squash_body(MergeMethod::Rebase, true, None, || unreachable!()).unwrap(),
            None
        );
    }

    /// The unreadable recorded-label probe reaches `pr ship` after the marker
    /// is released, so the retry names the human clear and the ship re-run.
    // trace:TASK-1951 | ai:claude
    #[test]
    fn task_1951_ship_unlabel_failure_names_the_ship_retry() {
        let line = ship_unlabel_failure(
            12,
            &crate::merge_hold::LabelSyncError::Other("could not read PR #12's labels".into()),
        );
        assert!(line.contains("could not read PR #12's labels"), "{line}");
        assert!(line.contains("aida merge-hold clear 12"), "{line}");
        assert!(line.contains("re-run `aida pr ship 12`"), "{line}");
    }

    #[test]
    fn cli_parses_rebase_and_merge_method_and_rejects_final_slice_with_squash() {
        // trace:STORY-1663.ac5970a5 | ai:antigravity
        let cli = crate::command_registry::try_parse_from(["aida", "pr", "ship", "42", "--rebase"])
            .expect("parses --rebase");
        match cli.command.registered::<crate::pr_cmd::PrArgs>() {
            crate::pr_cmd::PrArgs {
                inner:
                    crate::cli::PrCommand::Ship {
                        n,
                        rebase,
                        merge_method,
                        final_slice,
                        ..
                    },
            } => {
                assert_eq!(n, Some(42));
                assert!(rebase);
                assert!(merge_method.is_none());
                assert!(!final_slice);
                assert_eq!(
                    pr_ship::validate_ship_merge_method(rebase, merge_method, final_slice).unwrap(),
                    crate::forge::MergeMethod::Rebase
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli2 = crate::command_registry::try_parse_from([
            "aida",
            "pr",
            "ship",
            "42",
            "--merge-method",
            "rebase",
            "--final-slice",
        ])
        .expect("parses --merge-method rebase --final-slice");
        match cli2.command.registered::<crate::pr_cmd::PrArgs>() {
            crate::pr_cmd::PrArgs {
                inner:
                    crate::cli::PrCommand::Ship {
                        rebase,
                        merge_method,
                        final_slice,
                        ..
                    },
            } => {
                assert!(!rebase);
                assert_eq!(merge_method, Some(pr_ship::ShipMergeMethod::Rebase));
                assert!(final_slice);
                assert_eq!(
                    pr_ship::validate_ship_merge_method(rebase, merge_method, final_slice).unwrap(),
                    crate::forge::MergeMethod::Rebase
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli3 = crate::command_registry::try_parse_from([
            "aida",
            "pr",
            "ship",
            "42",
            "--rebase",
            "--complete-spec",
        ])
        .expect("parses --complete-spec alias");
        match cli3.command.registered::<crate::pr_cmd::PrArgs>() {
            crate::pr_cmd::PrArgs {
                inner:
                    crate::cli::PrCommand::Ship {
                        rebase,
                        final_slice,
                        ..
                    },
            } => {
                assert!(rebase);
                assert!(final_slice);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        // Conflicting --rebase and --merge-method is rejected by clap.
        assert!(crate::command_registry::try_parse_from([
            "aida",
            "pr",
            "ship",
            "42",
            "--rebase",
            "--merge-method",
            "squash"
        ])
        .is_err());

        // --final-slice without --rebase or --merge-method rebase is rejected.
        let err = pr_ship::validate_ship_merge_method(false, None, true)
            .expect_err("--final-slice without --rebase must be rejected");
        assert!(err.contains("requires `--rebase`"), "{err}");
        let err_squash = pr_ship::validate_ship_merge_method(
            false,
            Some(pr_ship::ShipMergeMethod::Squash),
            true,
        )
        .expect_err("--final-slice with --merge-method squash must be rejected");
        assert!(err_squash.contains("requires `--rebase`"), "{err_squash}");
    }

    #[test]
    fn rebase_ship_without_independent_approval_at_head_is_refused() {
        // trace:STORY-1663.ac28109e | ai:antigravity
        let tmp = tempfile::tempdir().unwrap();
        let policy = pr_ship::ShipApprovalPolicy::default();
        let err = pr_ship_approval_gate(
            tmp.path(),
            1490,
            &[],
            Some("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            &policy,
            false,
            true,
            None,
        )
        .expect_err("rebase ship uses the same approval gate and must refuse without verdict");
        assert_eq!(
            err.downcast_ref::<pr_ship::ShipFailure>().map(|f| f.reason),
            Some(pr_ship::ShipRefusal::Review)
        );
    }

    #[test]
    fn pr_ship_with_method_wires_identical_gates_and_rebase_hooks() {
        // trace:STORY-1663.ac28109e | ai:antigravity
        // trace:STORY-1663.ac3b41d5 | ai:antigravity
        let src = include_str!("pr_cmd.rs");
        let start = src
            .find(concat!("pub(crate) fn pr_ship_", "with_method("))
            .expect("pr_ship_with_method exists");
        let body = &src[start..];
        let preflight = body
            .find(concat!("ship_preflight_", "gates("))
            .expect("runs preflight gates");
        let stale_head = body
            .find(concat!("stale_head_after_", "ci_refusal("))
            .expect("checks stale head after CI");
        let approval_gate = body
            .find(concat!("let match_head = pr_ship_", "approval_gate("))
            .expect("runs approval gate");
        let verifier = body
            .find(concat!("run_optional_slice_stack_", "verifier("))
            .expect("runs optional slice-stack verifier");
        let head_pin = body
            .find(concat!("require_rebase_head_", "pin("))
            .expect("requires rebase head pin");
        let merge_call = body
            .find(concat!(".merge_", "change("))
            .expect("invokes merge_change");
        assert!(
            preflight < stale_head
                && stale_head < approval_gate
                && approval_gate < verifier
                && verifier < head_pin
                && head_pin < merge_call,
            "expected preflight -> stale_head -> approval_gate -> verifier -> head_pin -> merge_change order"
        );
    }

    #[test]
    fn rebase_ship_refuses_stale_head_and_missing_head_pin() {
        // trace:STORY-1663.ac3b41d5 | ai:antigravity
        let stale = pr_ship::stale_head_after_ci_refusal(
            1490,
            Some("1111111111111111111111111111111111111111"),
            Some("2222222222222222222222222222222222222222"),
        )
        .expect("head movement after preflight/CI must refuse");
        assert!(stale.contains("PR-1490"), "{stale}");
        assert!(stale.contains("moved"), "{stale}");
        // TASK-1623: a head that could not be read refuses instead of
        // skipping the check. trace:TASK-1951 trace:TASK-1623 | ai:claude
        let h = Some("1111111111111111111111111111111111111111");
        for (pre, post, at) in [
            (None, h, "preflight"),
            (Some(""), h, "preflight"),
            (h, None, "merge"),
            (h, Some("  "), "merge"),
        ] {
            let refusal = pr_ship::stale_head_after_ci_refusal(1490, pre, post)
                .expect("an unreadable head must refuse");
            assert!(
                refusal.contains(&format!("could not be read at {at}")),
                "{refusal}"
            );
        }
        assert!(pr_ship::stale_head_after_ci_refusal(1490, h, h).is_none());

        let unpinned =
            pr_ship::require_rebase_head_pin(1490, crate::forge::MergeMethod::Rebase, None)
                .expect_err("rebase merge without --match-head-commit pin must refuse");
        assert!(
            unpinned.contains("without a pinned head commit"),
            "{unpinned}"
        );

        assert!(pr_ship::require_rebase_head_pin(
            1490,
            crate::forge::MergeMethod::Rebase,
            Some("1111111111111111111111111111111111111111"),
        )
        .is_ok());
    }

    #[test]
    fn rebase_ship_slice_leaves_spec_status_unchanged_and_completes_only_on_final_slice() {
        // trace:STORY-1663.ac5970a5 | ai:antigravity
        let mut store = aida_core::RequirementsStore::default();
        let mut req = aida_core::Requirement::new(
            "Slice stack parent".to_string(),
            "Parent spec".to_string(),
        );
        req.spec_id = Some("STORY-1663".to_string());
        req.status = aida_core::RequirementStatus::InProgress;
        store.requirements.push(req.clone());

        // Default rebase ship (intermediate slice): suppresses completion trailer IDs
        // so the owning spec's status stays unchanged.
        let default_rebase_trailer = ship_completion_trailer_for_method(
            &store,
            crate::forge::MergeMethod::Rebase,
            false,
            "[1/3] feat(ship): slice one (STORY-1663)",
            "claude/story-1663-part1",
            "",
            false,
        );
        assert!(
            default_rebase_trailer.ids.is_empty(),
            "intermediate rebase slice must not complete owning spec: {default_rebase_trailer:?}"
        );
        assert!(
            default_rebase_trailer
                .notes
                .iter()
                .any(|n| n.contains("--final-slice")),
            "expected note explaining --final-slice: {:?}",
            default_rebase_trailer.notes
        );

        // Final-slice rebase ship (`--final-slice` / `--complete-spec`): resolves the
        // owning spec ID so `complete_rebase_final_slice_specs` transitions it to Completed.
        let final_slice_trailer = ship_completion_trailer_for_method(
            &store,
            crate::forge::MergeMethod::Rebase,
            true,
            "[3/3] feat(ship): slice three (STORY-1663)",
            "claude/story-1663-part3",
            "",
            false,
        );
        assert_eq!(final_slice_trailer.ids, vec!["STORY-1663".to_string()]);

        // Slice commit subject classifiers recognize both STORY-1488 moves and `[k/N]` slices.
        assert!(pr_ship::is_non_completing_slice_commit_subject(
            "refactor(cli-lib): extract pr_cmd.rs (STORY-1488)"
        ));
        assert!(pr_ship::is_non_completing_slice_commit_subject(
            "[1/3] feat(ship): slice one (STORY-1663)"
        ));
        assert!(!pr_ship::is_non_completing_slice_commit_subject(
            "feat(ship): standalone feature (STORY-1663)"
        ));

        // Verify in a real git repo + store that a non-final rebase slice with a
        // trailing `(STORY-1663)` commit subject leaves an `InProgress` spec
        // unchanged across `auto_bump_done_to_completed` (while an unrelated Done
        // spec `BUG-999` still auto-bumps to Completed), and `complete_rebase_final_slice_specs`
        // transitions `STORY-1663` to `Completed` on the final slice.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let run_git = |args: &[&str]| {
            let status = crate::test_git::git_command()
                .current_dir(root)
                .args(args)
                .status()
                .expect("git runs");
            assert!(status.success(), "git {args:?} failed");
        };
        run_git(&["init", "-b", "main"]);
        run_git(&["config", "user.name", "Test"]);
        run_git(&["config", "user.email", "test@example.com"]);
        std::fs::write(root.join("README.md"), "init\n").unwrap();
        run_git(&["add", "README.md"]);
        run_git(&["commit", "-m", "chore: initial"]);

        let store_dir = root.join(".aida-store");
        let backend = crate::test_env::home_fixture_backend(&store_dir).unwrap();
        use aida_core::db::DatabaseBackend;
        backend.add_requirement(req).unwrap();
        let mut other_req = aida_core::Requirement::new(
            "Unrelated done bug".to_string(),
            "Unrelated spec".to_string(),
        );
        other_req.spec_id = Some("BUG-999".to_string());
        other_req.status = aida_core::RequirementStatus::Done;
        backend.add_requirement(other_req).unwrap();

        // Simulate a non-final rebase slice commit and an unrelated completing commit on main.
        std::fs::write(root.join("README.md"), "slice 1\n").unwrap();
        run_git(&["add", "README.md"]);
        run_git(&["commit", "-m", "feat(ship): slice one (STORY-1663)"]);
        let slice_sha = String::from_utf8(
            crate::test_git::git_command()
                .current_dir(root)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        record_non_completing_rebase_slice(
            root,
            root,
            1490,
            "main",
            Some(&slice_sha),
            "claude/story-1663-part1",
            &[],
        )
        .expect("marker recorded");

        std::fs::write(root.join("README.md"), "slice 1 + bug 999\n").unwrap();
        run_git(&["add", "README.md"]);
        run_git(&["commit", "-m", "fix: unrelated bug (BUG-999) (#1491)"]);

        let storage = crate::Storage::new(&store_dir);
        let flips =
            crate::db_reconcile::auto_bump_done_to_completed(root, &store_dir, None, &storage)
                .expect("auto_bump runs");
        assert!(
            flips.iter().any(|f| f.spec_id == "BUG-999"),
            "unrelated spec must still auto-bump: {flips:?}"
        );
        assert!(
            !flips.iter().any(|f| f.spec_id == "STORY-1663"),
            "non-completing rebase slice must not auto-bump STORY-1663: {flips:?}"
        );
        let after_slice = backend
            .get_requirement_unambiguous("STORY-1663")
            .unwrap()
            .unwrap();
        assert_eq!(
            after_slice.status,
            aida_core::RequirementStatus::InProgress,
            "STORY-1663 status must stay unchanged after non-final slice"
        );

        // Now complete STORY-1663 via `--final-slice`.
        complete_rebase_final_slice_specs(
            root,
            1492,
            &["STORY-1663".to_string()],
            Some(&slice_sha),
        );
        let after_final = backend
            .get_requirement_unambiguous("STORY-1663")
            .unwrap()
            .unwrap();
        assert_eq!(
            after_final.status,
            aida_core::RequirementStatus::Completed,
            "STORY-1663 must transition to Completed on --final-slice"
        );
        assert_eq!(
            after_final
                .implementation_info
                .as_ref()
                .and_then(|i| i.completion_sha.as_deref()),
            Some(slice_sha.as_str())
        );
    }

    #[test]
    fn rebase_dry_run_plan_renders_gh_pr_merge_rebase_with_match_head_commit() {
        // trace:STORY-1663.ac3b41d5 | ai:antigravity
        let opts = pr_ship::PrShipOptions {
            pr_number: Some(1490),
            no_pull: false,
            no_cleanup: false,
            dry_run: true,
            complexity: None,
            effort: None,
        };
        let steps = vec![
            pr_ship::ShipStep::ResolvePr {
                create_if_needed: false,
            },
            pr_ship::ShipStep::WatchCi,
            pr_ship::ShipStep::Merge {
                delete_branch: true,
            },
            pr_ship::ShipStep::Pull,
            pr_ship::ShipStep::EndLease,
        ];
        let plan = pr_ship::format_dry_run_plan_with_method(
            &opts,
            &steps,
            crate::forge::ForgeKind::GitHub,
            crate::forge::MergeMethod::Rebase,
        );
        assert!(
            plan.contains(
                "gh pr merge <N> --rebase --delete-branch --match-head-commit <approved-head-sha>"
            ),
            "unexpected rebase dry-run plan:\n{plan}"
        );
    }

    #[test]
    fn slice_stack_verifier_runs_real_verify_stack_py_and_fails_closed_on_missing_range() {
        // trace:STORY-1663.ac28109e | ai:antigravity
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let run_git = |args: &[&str]| {
            let status = crate::test_git::git_command()
                .current_dir(root)
                .args(args)
                .status()
                .expect("git runs");
            assert!(status.success(), "git {args:?} failed");
        };
        run_git(&["init", "-b", "main"]);
        run_git(&["config", "user.name", "Test"]);
        run_git(&["config", "user.email", "test@example.com"]);

        let script_dir = root.join("scripts/split-lib");
        std::fs::create_dir_all(&script_dir).unwrap();
        let real_verifier = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("scripts/split-lib/verify_stack.py");
        std::fs::copy(&real_verifier, script_dir.join("verify_stack.py")).unwrap();
        run_git(&["add", "scripts/split-lib/verify_stack.py"]);
        run_git(&["commit", "-m", "chore: add verify_stack.py"]);
        run_git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);

        // 1. Fail-closed when the commit range cannot be inspected (e.g., bad head ref).
        let range_err = run_optional_slice_stack_verifier(
            root,
            root,
            1490,
            "origin/main",
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        )
        .expect_err("unresolvable range must fail closed when verifier is present");
        assert_eq!(
            range_err
                .downcast_ref::<pr_ship::ShipFailure>()
                .map(|f| f.reason),
            Some(pr_ship::ShipRefusal::Review)
        );
        assert!(
            range_err
                .to_string()
                .contains("could not inspect commit range"),
            "{range_err}"
        );

        // 2. Non-slice commits pass without invoking the slice-stack verifier.
        std::fs::write(root.join("README.md"), "normal change\n").unwrap();
        run_git(&["add", "README.md"]);
        run_git(&["commit", "-m", "feat: normal change"]);
        run_optional_slice_stack_verifier(root, root, 1490, "origin/main", "HEAD")
            .expect("non-slice commit range should pass");

        // 3. A slice-stack commit (`refactor(cli-lib): extract foo.rs`) invokes the
        // real `verify_stack.py <base> <head>` from `origin/main` and refuses on FAIL.
        std::fs::write(root.join("README.md"), "invalid slice move\n").unwrap();
        run_git(&["add", "README.md"]);
        run_git(&["commit", "-m", "refactor(cli-lib): extract foo.rs"]);
        let err = run_optional_slice_stack_verifier(root, root, 1490, "origin/main", "HEAD")
            .expect_err("invalid slice commit must fail real verify_stack.py");
        assert_eq!(
            err.downcast_ref::<pr_ship::ShipFailure>().map(|f| f.reason),
            Some(pr_ship::ShipRefusal::Review)
        );
        let msg = err.to_string();
        assert!(
            msg.contains("scripts/split-lib/verify_stack.py origin/main HEAD"),
            "{msg}"
        );
        assert!(msg.contains("FAIL"), "{msg}");
    }

    #[test]
    fn two_seat_docs_retire_slice_stack_exception_and_point_at_rebase_ship() {
        // trace:STORY-1663.ac82754a | ai:antigravity
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        for rel in [
            ".aida/discipline/two-seat-protocol.md",
            "aida-core/templates/.aida/discipline/two-seat-protocol.md",
        ] {
            let raw = std::fs::read_to_string(repo_root.join(rel)).unwrap();
            let doc = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                doc.contains("aida pr ship <n> --rebase"),
                "{rel} must point at `aida pr ship <n> --rebase`"
            );
            assert!(
                doc.contains("Raw `gh pr merge --rebase` is no longer an exception"),
                "{rel} must state that raw `gh pr merge --rebase` is no longer an exception"
            );
            assert!(
                doc.contains("leaves the owning spec's status unchanged"),
                "{rel} must state that rebase ship leaves the owning spec's status unchanged"
            );
        }
    }
}

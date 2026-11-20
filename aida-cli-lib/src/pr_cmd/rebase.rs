use super::*;

/// Mode selector for `aida pr rebase`. Computed once from the CLI
/// flags so the rest of the handler is a flat match on intent.
// trace:TASK-308 | ai:claude
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

/// Remove `aida pr rebase`'s temp worktree through the mail-retention guard;
/// a mail-less tree takes the fast path. The final quiet check runs right
/// before the destructive step, so nothing is ever deleted unfenced (R3-F3):
///
/// - a registered tree goes through `git worktree remove --force`, unlocked
///   first (a locked leftover, R4-F3);
/// - a directory Git no longer knows as a worktree (an interrupted run, a
///   prune) is deleted with `remove_dir_all`, inside the same fence. One with
///   no mail source of its own goes as on main; one with a `.aida/` cannot be
///   fenced (no anchor) and is kept with its mail.
///
/// A refusal or failure keeps the tree and says how to clear it
/// ([`REBASE_LEFTOVER_RECOVERY`]).
// trace:STORY-1843 | ai:claude
pub(crate) fn remove_rebase_worktree(
    project_root: &std::path::Path,
    wt_path: &std::path::Path,
) -> Result<()> {
    use aida_core::mailbox_lifecycle::guard;
    let reason = "pr rebase temp worktree";
    let registered = {
        let target = wt_path
            .canonicalize()
            .unwrap_or_else(|_| wt_path.to_path_buf());
        aida_core::git_ops::list_worktree_paths(project_root)
            .iter()
            .any(|p| p.canonicalize().unwrap_or_else(|_| p.clone()) == target)
    };
    let retirement = guard::retire_checkout(wt_path, reason)?;
    if registered {
        guard::remove_retired(
            wt_path,
            retirement,
            || {
                let _ = aida_core::git_ops::host_git_command(project_root)
                    .args(["worktree", "unlock", "--"])
                    .arg(wt_path)
                    .output();
                aida_core::git_ops::remove_worktree_unguarded(project_root, wt_path, true)
            },
            guard::Retirement::complete,
        )
    } else {
        guard::remove_retired(
            wt_path,
            retirement,
            || {
                std::fs::remove_dir_all(wt_path)
                    .with_context(|| format!("remove the leftover {}", wt_path.display()))
            },
            guard::Retirement::complete,
        )
    }
}

/// How to clear a rebase temp tree [`remove_rebase_worktree`] kept: its
/// `.aida/` holds that tree's mail, and once it is moved out the tree owns no
/// mail source, so the next run removes it.
pub(crate) const REBASE_LEFTOVER_RECOVERY: &str =
    "move its `.aida/` directory (it holds that tree's mail) somewhere safe; the next \
     `aida pr rebase` then removes the tree";

/// The temp worktree path for this run. A leftover from an earlier run is
/// removed through the guard; when it has to be kept (it holds mail, or
/// cannot be fenced), this run uses a fresh sibling path instead, so a kept
/// leftover never blocks later rebases (R4-F3).
// trace:STORY-1843 | ai:claude
pub(crate) fn rebase_worktree_for_run(
    project_root: &std::path::Path,
    n: u64,
) -> std::path::PathBuf {
    let wt_path = crate::pr_rebase::temp_worktree_path(project_root, n);
    if !wt_path.exists() {
        return wt_path;
    }
    let Err(e) = remove_rebase_worktree(project_root, &wt_path) else {
        return wt_path;
    };
    eprintln!(
        "warning: a previous rebase left {}, and it was kept ({e:#}); to clear it, {}",
        wt_path.display(),
        REBASE_LEFTOVER_RECOVERY
    );
    let mut name = wt_path.file_name().unwrap_or_default().to_os_string();
    name.push(format!("-{}", std::process::id()));
    let fresh = wt_path.with_file_name(name);
    if fresh.exists() {
        let _ = remove_rebase_worktree(project_root, &fresh);
    }
    fresh
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
// trace:BUG-289 | ai:claude
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
// trace:TASK-308 | ai:claude
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
        resolve_smoke_check,
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
    // trace:BUG-2618 | ai:codex
    if !check {
        let key = format!(
            "{}-{n}",
            crate::forge::resolve_forge_kind(&project_root).change_noun()
        );
        crate::pr_head_guard::check(&project_root, &key, n, &info.head_oid)?;
    }

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
    // A leftover from a previous interrupted run is removed behind the
    // mail-retention guard, or kept and stepped around. trace:STORY-1843 | ai:claude
    let wt_path = rebase_worktree_for_run(&project_root, n);
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
        if let Err(e) = remove_rebase_worktree(&project_root, &wt_path) {
            eprintln!(
                "warning: kept the rebase worktree {}: {e:#}; to clear it, {}",
                wt_path.display(),
                REBASE_LEFTOVER_RECOVERY
            );
        }
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

    // Re-read approval and reservations immediately before publication.
    // trace:BUG-2618 | ai:codex
    let key = format!(
        "{}-{n}",
        crate::forge::resolve_forge_kind(&project_root).change_noun()
    );
    crate::pr_head_guard::check(&project_root, &key, n, &info.head_oid)?;

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

/// `aida pr rebase <N> --check` — print the stale-base / overlap /
/// conflict-prediction report and exit zero. Modifies nothing.
// trace:TASK-308 | ai:claude
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
/// detect always blocks.
// trace:STORY-281 | ai:claude
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

    // Fast freshness classifies like any other change: the caller applies the
    // project's `[review] stale_base` policy to this outcome, and a refused
    // fast conflict is then routed by `review_launch::route_refused_conflict`.
    // trace:STORY-2763 | ai:claude
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
// trace:TASK-308 | ai:claude
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
// trace:TASK-480 | ai:claude
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
/// isolation.
///
/// Every path handed in here comes from a PR's commit range, so it is
/// committed (tracked) by construction — even one the PR adds that the
/// local index has never seen. It is therefore judged only by the
/// `.gitignore` rules in the tree, never by this clone's
/// `info/exclude` or the user's global `core.excludesFile`.
// trace:TASK-480 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-009 | ai:claude
pub(crate) fn classify_intermediate_only_with_gitignore(
    project_root: &std::path::Path,
    changed: &[String],
) -> pr_rebase::IntermediateOnlyOutcome {
    pr_rebase::classify_intermediate_only(changed, |path| {
        git_path_is_ignored_by_tree_rules(project_root, path)
    })
}

/// Whether `path` is in the index of `project_root`.
// trace:BUG-b6b1278890774ef993d2ebc327862470-009 | ai:claude
fn git_path_is_tracked(project_root: &std::path::Path, path: &str) -> bool {
    aida_core::git_ops::host_git_command(project_root)
        .args(["ls-files", "-z", "--"])
        .arg(format!(":(literal){path}"))
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// `git ls-files -c -i --exclude-per-directory=.gitignore` for an indexed
/// path: only in-tree `.gitignore` files are read, never `info/exclude`
/// or `core.excludesFile`.
// trace:BUG-b6b1278890774ef993d2ebc327862470-009 | ai:claude
fn git_indexed_path_is_ignored_by_tree_rules(project_root: &std::path::Path, path: &str) -> bool {
    aida_core::git_ops::host_git_command(project_root)
        .args([
            "ls-files",
            "-z",
            "-c",
            "-i",
            "--exclude-per-directory=.gitignore",
            "--",
        ])
        .arg(format!(":(literal){path}"))
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Ignore check restricted to the in-tree `.gitignore` files (as checked
/// out in the working tree).
///
/// An indexed path is asked of `git ls-files -i` with only
/// `--exclude-per-directory=.gitignore`, which never reads `info/exclude`
/// or `core.excludesFile` — so a local rule on an ancestor directory
/// cannot mask a committed `.gitignore` match.
///
/// A path the local index has never seen (added by the PR) falls back to
/// `check-ignore -v`: it is ignored only when the reported deciding
/// source is an in-tree `.gitignore` and the pattern is not a negation
/// (`-v` reports negated matches too). Limitation: git resolves ancestor
/// directory exclusion first, so when `info/exclude` or
/// `core.excludesFile` matches an ancestor directory of such a path, that
/// local source is reported and the path reads as not-ignored (fail open).
// trace:BUG-b6b1278890774ef993d2ebc327862470-009 | ai:claude
pub(crate) fn git_path_is_ignored_by_tree_rules(
    project_root: &std::path::Path,
    path: &str,
) -> bool {
    if git_path_is_tracked(project_root, path) {
        return git_indexed_path_is_ignored_by_tree_rules(project_root, path);
    }
    use std::io::Write as _;
    // `-z` requires `--stdin`; NUL framing keeps odd paths unquoted.
    let Ok(mut child) = aida_core::git_ops::host_git_command(project_root)
        .args(["check-ignore", "-v", "-z", "--stdin", "--no-index"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(path.as_bytes());
        let _ = stdin.write_all(b"\0");
    }
    let Ok(out) = child.wait_with_output() else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    // `-v -z` record: <source> NUL <linenum> NUL <pattern> NUL <path> NUL.
    let mut fields = out.stdout.split(|b| *b == 0);
    let source = String::from_utf8_lossy(fields.next().unwrap_or_default());
    let _linenum = fields.next();
    let pattern = fields.next().unwrap_or_default();
    let source = std::path::Path::new(source.as_ref());
    let from_tree_gitignore = !source.is_absolute()
        && source.file_name() == Some(std::ffi::OsStr::new(".gitignore"))
        && !source.starts_with(".git");
    from_tree_gitignore && !pattern.is_empty() && !pattern.starts_with(b"!")
}

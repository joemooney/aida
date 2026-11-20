use super::*;

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
pub(super) fn fetch_merged_pr_refs(
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
pub(super) fn authored_slice_start(
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
pub(super) fn confirm_rebase_merged(
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
pub(super) fn landed_commits_for_subjects(
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
pub(super) fn landed_run(
    log: &[(&str, &str)],
    subjects: &[String],
    oldest: bool,
) -> Option<Vec<String>> {
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
        // Messages show the friendly label; the completion record keeps the
        // canonical id. trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
        let shown = req.friendly_id();
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
                    shown,
                    pr_number
                );
            }
            Ok(crate::completion::AtomicCompletion::AlreadyCompleted) => {
                eprintln!(
                    "  {} {} is already Completed (PR-{})",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    shown,
                    pr_number
                );
            }
            Ok(crate::completion::AtomicCompletion::Refused(status)) => {
                eprintln!(
                    "  {} could not complete {} after PR-{}: spec is in terminal status {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    shown,
                    pr_number,
                    status
                );
            }
            Ok(crate::completion::AtomicCompletion::Gone) => {
                eprintln!(
                    "  {} could not complete {} after PR-{}: spec disappeared before write",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    shown,
                    pr_number
                );
            }
            Err(e) => {
                eprintln!(
                    "  {} could not complete {} after final-slice rebase merge of PR-{}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    shown,
                    pr_number
                );
            }
        }
    }
}

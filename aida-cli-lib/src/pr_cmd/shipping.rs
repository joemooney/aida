use super::*;

/// `gh pr create` with title/body derived from the latest commit on
/// `branch`. Returns the new PR's number, parsed from the URL `gh`
/// prints on success. Branch must already be pushed; we push it first.
/// BUG-574: return the most-recent MERGED PR number for `branch`, if any.
/// Used by `aida pr ship` to recognize an already-shipped branch (no open PR
/// but a merged one) and exit 0 instead of failing in `pr_ship_create_pr`'s
/// "no commits between" path. Best-effort: any `gh` failure (missing binary,
/// auth, network) returns `None`, so a probe blip never converts an otherwise-
/// fine create into a spurious already-merged short-circuit.
// trace:BUG-574 | ai:claude
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

    // trace:BUG-2618 | ai:codex
    crate::pr_head_guard::check_branch(project_root, branch)?;
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
    // STORY-080 AC2: verdicts recorded for the credited specs before this
    // PR existed bind to it, so the change answers for them.
    // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
    let change = crate::gate::ledger::open_change_bound(
        &*crate::forge::forge_for_open_change(project_root),
        project_root,
        req,
        &extract_spec_ids_from_commit(&commit_msg),
    )
    .map_err(crate::forge::open_change_error)?;
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

/// The squash body `aida pr ship` hands the forge: `body` (built by
/// [`crate::gate::trailer_coverage::ship_trailer_check`] for every squash
/// ship, never left to the forge) carrying, on a non-completing ship, the
/// `aida: no-complete` declaration. Other merge methods pass no body.
///
/// TASK-2116 / BUG-HOME-048 trade-off: an explicit body replaces the
/// forge's default one, so GitHub no longer derives `Co-authored-by:`
/// trailers from the other commit authors. Trailers already written in
/// commit messages are kept. Every squash ship accepts this: the forge's
/// default body copies each branch commit message verbatim, and the
/// pull-side scan would complete any uncovered trailer it found there.
// trace:TASK-1951 trace:TASK-1765 trace:TASK-2116 trace:TASK-2117 | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
pub(crate) fn ship_squash_body(
    method: crate::forge::MergeMethod,
    declares_non_completing: bool,
    body: Option<String>,
) -> Option<String> {
    if method != crate::forge::MergeMethod::Squash {
        return None;
    }
    let body = body?;
    Some(if declares_non_completing {
        non_completing_squash_body(&body)
    } else {
        body
    })
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
            return Err(crate::gate::ship::ShipRefusal::Review.error(refusal));
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
        return Err(crate::gate::ship::ShipRefusal::Review.error(refusal));
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
        return Err(crate::gate::ship::ShipRefusal::Review.error(refusal));
    }
    Ok(())
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
) -> Vec<crate::gate::ship::OpenApprovedPr> {
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
    crate::gate::ship::parse_open_approved_prs_json(&String::from_utf8_lossy(&out.stdout))
}

/// STORY-1917 Q2: the points inside `pr ship` where a dispatch-originated
/// ship re-checks its request's authority (original mandate, grants, human
/// gates, claim) before going on. Ordinary ships carry no context and are
/// unaffected.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShipCheckpoint {
    /// Before anything else runs.
    Entry,
    /// After the PR is resolved, before the mandate cover is taken.
    PreMandateCover,
    /// After the CI wait.
    PostCiWait,
    /// Under the merge lease, before the mandate is re-read.
    UnderMergeLease,
    /// Immediately before a merge-hold is released.
    PreHoldRelease,
    /// Immediately before the merge call, after every wait.
    PreMerge,
}

#[cfg(test)]
type ShipCheckpointObserver = Box<dyn FnMut(ShipCheckpoint)>;

#[cfg(test)]
std::thread_local! {
    /// Runs at each [`ShipCheckpoint`] just before the authority check, so a
    /// test can change authority at that exact point of a real ship.
    pub(crate) static TEST_SHIP_CHECKPOINT_OBSERVER: std::cell::RefCell<Option<ShipCheckpointObserver>> =
        std::cell::RefCell::new(None);
}

/// Re-check a dispatch-originated ship's authority at `at`; a no-op for an
/// ordinary ship, and a refusal when a dispatch request executing on this
/// thread reached `pr ship` without its context. The error is a typed
/// [`crate::gate::dispatch_auth::DispatchAuthorityAbortedError`].
// trace:STORY-1917 trace:TASK-2344 | ai:claude
pub(crate) fn dispatch_ship_checkpoint(
    dispatch_ctx: Option<&crate::gate::dispatch_auth::DispatchExecutionContext>,
    at: ShipCheckpoint,
) -> Result<()> {
    let Some(ctx) = crate::gate::dispatch_auth::carried(dispatch_ctx, "pr ship")? else {
        return Ok(());
    };
    #[cfg(test)]
    TEST_SHIP_CHECKPOINT_OBSERVER.with(|cell| {
        if let Some(observe) = cell.borrow_mut().as_mut() {
            observe(at);
        }
    });
    ctx.reverify_now(true)
}

/// [`dispatch_ship_checkpoint`] for a checkpoint after the PR is known and
/// before the pre-merge one: a refusal also writes a `pr:<N>` row under the
/// request's own mandate, as the pre-merge refusal does, so every dispatch
/// stop inside `pr ship` is ledgered and not only the last. The error keeps
/// its type.
// trace:STORY-1917 | ai:claude
pub(crate) fn dispatch_ship_checkpoint_ledgered(
    root: &std::path::Path,
    dispatch_ctx: Option<&crate::gate::dispatch_auth::DispatchExecutionContext>,
    pr: u64,
    at: ShipCheckpoint,
) -> Result<()> {
    let Err(err) = dispatch_ship_checkpoint(dispatch_ctx, at) else {
        return Ok(());
    };
    if let Some(ctx) = dispatch_ctx {
        let why = format!(
            "PR-{pr} left OPEN at the {at:?} check — its dispatch request no longer has authority to ship ({err:#})"
        );
        ledger_mandate_ship_failure(root, Some(&dispatch_ledger_cover(ctx)), pr, &why);
    }
    Err(err)
}

/// Appended to a pre-merge dispatch refusal for a PR whose hold this ship did
/// not release.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
pub(crate) const NO_HOLD_RELEASED_NOTE: &str =
    " No merge-hold was released for it, so there is nothing to re-arm.";

/// The dispatch request's own mandate, for ledgering a refusal on a PR that
/// had no mandate-released hold.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
pub(crate) fn dispatch_ledger_cover(
    ctx: &crate::gate::dispatch_auth::DispatchExecutionContext,
) -> MandateCover {
    MandateCover {
        gate_match: None,
        issued_supervised: false,
        supervised_by_mode: false,
        checked_hold: None,
        checked_head: None,
        id: ctx.mandate_id().to_string(),
        principal: ctx.principal().to_string(),
    }
}

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
            crate::gate::dispatch_types::Outcome::Refused,
            Some(match cover.gate_audit() {
                Some(audit) => format!("{why}\n{audit}"),
                None => why.to_string(),
            }),
        );
    }
}

/// BUG-417: return `Some(reason)` when `aida pr ship` should skip the blocking
/// CI wait rather than hang on a repo that will never register checks. Two
/// triggers: (1) an explicit opt-out env (`AIDA_PR_SHIP_NO_CI_WAIT=1` or the
/// `lifecycle:no-ci-wait` token in `AIDA_LIFECYCLE_TAGS`); (2) neither GitHub
/// Actions workflows nor `.gitlab-ci.yml` configured. `None` ⇒ wait normally.
// trace:BUG-417 | ai:claude
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
    // A review-gated project merges on an approval at the exact head; CI
    // runs after the merge. trace:STORY-2897 | ai:claude
    if !crate::landing_policy::load(project_root).waits_for_ci() {
        return Some(
            "landing mode is review-gated (`[landing] mode`): the merge waits on an \
             approval at the exact head, and CI runs after it"
                .to_string(),
        );
    }

    // No workflows configured ⇒ checks will never register.
    if !repo_has_ci_workflows(project_root) {
        return Some(
            "no CI workflows configured (.github/workflows or .gitlab-ci.yml)".to_string(),
        );
    }
    None
}

/// True when the project has a GitLab CI file or at least one GitHub Actions workflow file
/// (`.github/workflows/*.{yml,yaml}`). The pure file-name rule lives in
/// `pr_ship::workflow_files_indicate_ci`; this only does the directory read.
// trace:BUG-417 | ai:claude
pub(crate) fn repo_has_ci_workflows(project_root: &std::path::Path) -> bool {
    // trace:BUG-2515 | ai:codex
    if project_root.join(".gitlab-ci.yml").is_file() {
        return true;
    }
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
// trace:BUG-434 | ai:claude
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

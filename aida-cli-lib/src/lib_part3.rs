/// BUG-1468: a PR branch's green check is evidence about the guards that
/// existed WHEN IT RAN. Nothing re-evaluates that green when a new required
/// guard lands on main, so a long-lived branch can present a green check
/// that no longer means what a reader assumes (observed on PR #1979 —
/// `aida-core/templates/.aida/discipline/session-discipline.md` grew a
/// content check 15h after the branch's own CI ran, and the branch still
/// showed green).
///
/// TWO TIERS (BUG-1468 follow-up — the single-tier version refused almost
/// every ship in this repo, because nearly every commit on main touches
/// `*/tests/` or `*_tests.rs`, making `--override-stale-check` routine):
/// - `definition_files`: a `.github/workflows/*` file whose `on:` triggers
///   include `pull_request` (a plain substring check on the file's content
///   at `base_ref` — nightly/cron-only, release-only, and dispatch-only
///   workflows don't gate a PR's own check, so they do NOT count), or a
///   `scripts/` file one of THOSE PR-triggered workflows invokes directly —
///   the check's own DEFINITION changed, so the green no longer means what
///   it looks like. `aida pr ship` REFUSES on this tier (override-able); the
///   drain's merge phase only WARNS and proceeds (BUG-1468 follow-up 2).
/// - `test_files`: WARN-only, everywhere. An ordinary test file changed on
///   base since divergence — the common, usually-harmless "base moved" case;
///   still worth surfacing (a reader may want to re-run), never worth
///   refusing.
// trace:BUG-1468 | ai:claude
pub(crate) struct StaleCheckWarning {
    pub(crate) behind_commits: u64,
    pub(crate) definition_files: Vec<String>,
    pub(crate) test_files: Vec<String>,
}

/// Pure classifier for the two tiers above. `workflow_is_pr_triggered`
/// decides whether a changed `.github/workflows/...` file's `on:` triggers
/// include `pull_request` (a non-PR-triggered workflow — cron/nightly,
/// release, manual dispatch — does NOT count as a definition change at
/// all: it lands in neither tier). `script_is_pr_workflow_invoked` decides
/// whether a `scripts/...` path is one a PR-triggered workflow calls
/// directly. Callers resolve both via git on `base_ref` (tests fake them
/// directly, no git needed).
// trace:BUG-1468 | ai:claude
pub(crate) fn classify_changed_files(
    changed_files: &[String],
    workflow_is_pr_triggered: &dyn Fn(&str) -> bool,
    script_is_pr_workflow_invoked: &dyn Fn(&str) -> bool,
) -> (Vec<String>, Vec<String>) {
    let mut definition = Vec::new();
    let mut test_only = Vec::new();
    for f in changed_files {
        if is_workflow_path(f) {
            if workflow_is_pr_triggered(f) {
                definition.push(f.clone());
            }
            // else: a nightly/cron/dispatch/release-only workflow — its
            // green never covered this PR's check to begin with, so it
            // doesn't count in either tier.
        } else if is_script_path(f) && script_is_pr_workflow_invoked(f) {
            definition.push(f.clone());
        } else if is_test_path(f) {
            test_only.push(f.clone());
        }
    }
    (definition, test_only)
}

// trace:BUG-1468 | ai:claude
pub(crate) fn is_workflow_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with(".github/workflows/") || lower.contains("/.github/workflows/")
}

// trace:BUG-1468 | ai:claude
pub(crate) fn is_script_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("scripts/") || lower.contains("/scripts/")
}

// trace:BUG-1468 | ai:claude
pub(crate) fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("tests/")
        || lower.contains("/tests/")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_tests.rs")
        || lower.ends_with("_test.py")
        || lower.ends_with("_test.sh")
}

/// True when a `.github/workflows/...` file's content, AT `base_ref`,
/// mentions `pull_request` — a plain substring check (no YAML parsing),
/// standing in for "this workflow's `on:` triggers include `pull_request`"
/// (BUG-1468 follow-up 2). A nightly/cron, release, or manual-dispatch-only
/// workflow never gates a PR's own check, so it must not count as a
/// definition change even though it lives under `.github/workflows/`.
/// False on any git error (fail-open — a git hiccup demotes a definition
/// change to "doesn't count" rather than blocking a ship on its own).
// trace:BUG-1468 | ai:claude
pub(crate) fn workflow_is_pr_triggered(
    repo: &std::path::Path,
    base_ref: &str,
    workflow_path: &str,
) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &format!("{base_ref}:{workflow_path}")])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("pull_request"))
        .unwrap_or(false)
}

/// The `.github/workflows/*` files, AT `base_ref`, whose content mentions
/// `pull_request` — the PR-triggered subset a `scripts/` change must be
/// invoked by to count as a definition change. Empty on any git error
/// (fail-open — see [`workflow_is_pr_triggered`]).
// trace:BUG-1468 | ai:claude
pub(crate) fn pr_triggered_workflow_files(repo: &std::path::Path, base_ref: &str) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "grep",
            "-l",
            "-F",
            "pull_request",
            base_ref,
            "--",
            ".github/workflows",
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    // `git grep -l <rev> -- <pathspec>` prints "<rev>:<path>" per match.
    let prefix = format!("{base_ref}:");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix(prefix.as_str()).map(|p| p.to_string()))
        .collect()
}

/// True when `script_path` (a `scripts/...` file that changed) is invoked
/// directly by one of the PR-TRIGGERED workflows on `base_ref` — a literal
/// substring match of the path inside those tracked `.github/workflows/*`
/// blobs. A workflow that invokes the same script but isn't itself
/// PR-triggered (nightly, release, dispatch-only) does not count (BUG-1468
/// follow-up 2). False on any git error (fail-open).
// trace:BUG-1468 | ai:claude
pub(crate) fn script_referenced_by_pr_workflows(
    repo: &std::path::Path,
    base_ref: &str,
    script_path: &str,
) -> bool {
    pr_triggered_workflow_files(repo, base_ref)
        .iter()
        .any(|wf| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["show", &format!("{base_ref}:{wf}")])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).contains(script_path))
                .unwrap_or(false)
        })
}

/// Resolve the ref to measure `branch`'s staleness FROM: `origin/<branch>`
/// when that remote-tracking ref exists (what a PR's own CI actually ran
/// against), else the local `branch` ref itself (BUG-1468 follow-up 3 — a
/// local checkout can be ahead or behind what's actually pushed/reviewed).
// trace:BUG-1468 | ai:claude
pub(crate) fn stale_check_branch_ref(repo: &std::path::Path, branch: &str) -> String {
    let remote_ref = format!("origin/{branch}");
    let exists = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &remote_ref])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if exists {
        remote_ref
    } else {
        branch.to_string()
    }
}

/// git-IO wrapper: how far `branch` is behind `base_ref`, and the two-tier
/// classification (see [`StaleCheckWarning`]) of what changed on `base_ref`
/// since divergence. Staleness is measured from `origin/<branch>` when that
/// ref exists, else the local `branch` ref (see [`stale_check_branch_ref`]).
/// `None` when the branch is not behind base (nothing to warn about) or on
/// a git error — same fail-open convention as [`branch_behind_main`], so a
/// git hiccup never blocks a ship.
// trace:BUG-1468 | ai:claude
pub(crate) fn pr_stale_check_warning(
    repo: &std::path::Path,
    branch: &str,
    base_ref: &str,
) -> Option<StaleCheckWarning> {
    let branch_ref = stale_check_branch_ref(repo, branch);
    let count_out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        // trace:BUG-1622 | ai:claude
        .args([
            "rev-list",
            "--count",
            git_arg_guard::END_OF_OPTIONS,
            &format!("{branch_ref}..{base_ref}"),
        ])
        .output()
        .ok()?;
    if !count_out.status.success() {
        return None;
    }
    let behind_commits: u64 = String::from_utf8_lossy(&count_out.stdout)
        .trim()
        .parse()
        .ok()?;
    if behind_commits == 0 {
        return None;
    }
    let diff_out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        // trace:BUG-1622 | ai:claude
        .args([
            "diff",
            "--name-only",
            git_arg_guard::END_OF_OPTIONS,
            &format!("{branch_ref}...{base_ref}"),
            "--",
        ])
        .output()
        .ok()?;
    if !diff_out.status.success() {
        return None;
    }
    let changed: Vec<String> = String::from_utf8_lossy(&diff_out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    let (definition_files, test_files) = classify_changed_files(
        &changed,
        &|workflow_path| workflow_is_pr_triggered(repo, base_ref, workflow_path),
        &|script_path| script_referenced_by_pr_workflows(repo, base_ref, script_path),
    );
    Some(StaleCheckWarning {
        behind_commits,
        definition_files,
        test_files,
    })
}

#[cfg(test)]
#[path = "tests/bug_1468_stale_check_tests.rs"]
mod bug_1468_stale_check_tests;

/// STORY-73: resolution chain for `aida session end` (no arg). Tries in
/// order, stopping at first hit:
///
///   1. cwd-based: lease whose worktree_path contains cwd. (existing flow)
///   2. AIDA_SESSION_ID env var: exported by `session start` when wrapped
///      via the shell helper; matches a live lease by id prefix.
///   3. ancestor PID: walks the calling shell's ancestors via sysinfo and
///      matches against any lease's `creator_pid`. Catches the common case
///      where the user ran `start` then `end` from the same shell, never
///      cd'd into the worktree, and AIDA_SESSION_ID isn't set (e.g., shell
///      helpers not installed).
///   4. single-active fallback: if exactly one lease is active for this
///      project, prompt y/N (or auto-accept with -y).
///   5. error: list active leases and ask for an explicit id.
///
// ----------------------------------------------------------------------------
// TASK-111: CI-aware `aida session end`.
//
// When a session has an associated PR (discovered via `gh pr list --head
// <branch>`), `aida session end` probes the PR's CI conclusion and chooses
// an action based on a small decision tree:
//
//   No PR / No gh         → Proceed silently (today's behavior preserved
//                            for non-PR sessions; gh-missing is graceful)
//   PR exists, no CI runs → Info: "PR opened, CI hasn't started" + proceed
//   PR exists, CI running → Prompt (default) / Wait (--wait-ci) / Cancel
//   PR exists, CI green   → Info: format!("{} CI green", crate::glyph(crate::glyphs::Glyph::Check)) + proceed
//   PR exists, CI red     → Warn + prompt to keep session for fixups
//
// Both `--skip-ci` and `--force` bypass the probe entirely. With `--yes`
// (non-interactive), running-CI defaults to proceed (you opted out of
// prompts; we don't strand the script). Red-CI with `--yes` still
// proceeds but the warning is loud.
//
// trace:TASK-111 | ai:claude
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CiProbe {
    /// No gh on PATH, or no PR for this branch, or gh call failed in a way
    /// we don't want to surface (network blip, auth issue) — degrade
    /// silently to today's behavior. The reason is included for the
    /// info-line when logging.
    NoSignal(String),
    /// PR exists but CI hasn't started (no workflow runs yet).
    PrNoChecks { pr_number: u32 },
    /// CI is in progress on the latest commit.
    InProgress { pr_number: u32 },
    /// All checks passed.
    Green { pr_number: u32 },
    /// At least one check failed.
    Red {
        pr_number: u32,
        failed_summary: String,
    },
}

// trace:BUG-1818 | ai:codex
pub(crate) fn ci_probe_change_id(probe: &CiProbe) -> Option<u32> {
    match probe {
        CiProbe::PrNoChecks { pr_number }
        | CiProbe::InProgress { pr_number }
        | CiProbe::Green { pr_number }
        | CiProbe::Red { pr_number, .. } => Some(*pr_number),
        CiProbe::NoSignal(_) => None,
    }
}

/// Pure policy for an unavailable CI probe while a wait is active.
/// Transient failures get the configured retry budget; permanent failures
/// close the wait immediately, and exhaustion returns `NoSignal` so the drain
/// gate can shelve with the typed `ci-unavailable` cause.
// trace:BUG-1250 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CiProbeFailureAction {
    Retry,
    Unavailable,
}

pub(crate) fn decide_ci_probe_failure(
    reason: &str,
    consecutive_failures: u32,
    max_attempts: u32,
    transient_patterns: &[String],
) -> CiProbeFailureAction {
    if crate::network_retry::classify_transient(reason, transient_patterns)
        && consecutive_failures < max_attempts.max(1)
    {
        CiProbeFailureAction::Retry
    } else {
        CiProbeFailureAction::Unavailable
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CiAction {
    /// Run the rest of session_end as today.
    Proceed,
    /// Block on a poll loop until CI reaches a terminal state, then
    /// re-decide.
    Wait,
    /// Refuse to end the session; the message includes the reason for
    /// the user.
    Cancel(String),
}

/// Pure decision function: given a probe result + flags, decide what to
/// do. Pure so the unit tests can pin every branch without spawning gh.
/// trace:TASK-111 | ai:claude
pub(crate) fn decide_ci_action(probe: &CiProbe, wait_ci: bool, yes: bool) -> CiAction {
    match probe {
        CiProbe::NoSignal(reason) => {
            // Stay quiet for the common "no PR yet" case; only surface
            // info when something interesting (gh missing, lookup failed)
            // would help the user notice.
            if !reason.is_empty() && !reason.contains("no open PR") {
                eprintln!(
                    "  {} {}",
                    "(ci-probe:".dimmed(),
                    format!("{})", reason).dimmed()
                );
            }
            CiAction::Proceed
        }
        CiProbe::PrNoChecks { pr_number } => {
            eprintln!(
                "  {} PR-{} opened, CI hasn't started yet — ending anyway.",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                pr_number,
            );
            CiAction::Proceed
        }
        CiProbe::Green { pr_number } => {
            eprintln!(
                "  {} CI green on PR-{}.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                pr_number
            );
            CiAction::Proceed
        }
        CiProbe::InProgress { pr_number } => {
            if wait_ci {
                // TASK-233: flag-neutral wording — the caller passes
                // `--wait-ci || --watch-ci` and prints the variant-
                // specific intro line itself.
                eprintln!(
                    "  {} CI in progress on PR-{} — blocking until CI completes.",
                    crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                    pr_number,
                );
                CiAction::Wait
            } else if yes {
                eprintln!(
                    "  {} CI in progress on PR-{} (proceeding — --yes set; pass --wait-ci to block).",
                    crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                    pr_number,
                );
                CiAction::Proceed
            } else {
                CiAction::Cancel(format!(
                    "  {} CI is still in progress on PR-{}.\n  Options:\n    --wait-ci   block until CI completes\n    --skip-ci   release lease now (you'll have to push fixups in a new session if CI goes red)\n    --force     release lease unconditionally",
                    crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                    pr_number,
                ))
            }
        }
        CiProbe::Red {
            pr_number,
            failed_summary,
        } => {
            if yes {
                eprintln!(
                    "  {} CI RED on PR-{}: {}\n  ({} set — ending anyway. Push fixups in a new session via `aida queue rework`.)",
                    crate::glyph(crate::glyphs::Glyph::Cross).red(),
                    pr_number,
                    failed_summary.dimmed(),
                    "--yes".dimmed(),
                );
                CiAction::Proceed
            } else {
                CiAction::Cancel(format!(
                    "  {} CI RED on PR-{}: {}\n  Recommended: keep this session alive and push fixups (the implementer's lease lets you commit without re-claiming).\n  To end anyway: pass --force or --yes.",
                    crate::glyph(crate::glyphs::Glyph::Cross).red(),
                    pr_number,
                    failed_summary,
                ))
            }
        }
    }
}

/// Side-effecting probe: shell out to gh and parse the result. Returns a
/// `CiProbe::NoSignal` for any failure path so callers don't have to
/// distinguish between "no PR" and "gh broken" — both degrade to "proceed
/// silently." trace:TASK-111 | ai:claude
/// STORY-516: forge-routed CI probe, so a GitLab / pure-git repo goes through
/// its own provider. `project_root` SELECTS the provider. TASK-1273: it is now
/// always supplied by the caller via `ci_probe_with_forge` — the old
/// `ci_probe_via_forge` wrapper resolved it from the process cwd, which routed
/// a GitLab branch through GitHub whenever the driven worktree was not the
/// agent's cwd. Converts the forge-neutral `CiProbeResult` back to `CiProbe` so
/// the existing match sites are unchanged; a provider Err collapses to
/// `NoSignal`.
/// STORY-516: reverse of `ci_probe_result_from_ci_probe` — convert a forge
/// `CiProbeResult` (incl. a provider `Err`) back to the orchestrator's
/// `CiProbe`, so the `*_via_forge` CI helpers stay a pure name-swap at their
/// call sites. trace:STORY-516 | ai:claude
// trace:TASK-1273 | ai:claude
pub(crate) fn ci_probe_from_ci_probe_result(r: Result<crate::forge::CiProbeResult>) -> CiProbe {
    match r {
        Ok(crate::forge::CiProbeResult::NoSignal(why)) => CiProbe::NoSignal(why),
        Ok(crate::forge::CiProbeResult::NoChecks { change }) if change > 0 => CiProbe::PrNoChecks {
            pr_number: change as u32,
        },
        Ok(crate::forge::CiProbeResult::InProgress { change }) if change > 0 => {
            CiProbe::InProgress {
                pr_number: change as u32,
            }
        }
        Ok(crate::forge::CiProbeResult::Green { change }) if change > 0 => CiProbe::Green {
            pr_number: change as u32,
        },
        Ok(crate::forge::CiProbeResult::Failed { change, summary }) if change > 0 => CiProbe::Red {
            pr_number: change as u32,
            failed_summary: summary,
        },
        Ok(_) => CiProbe::NoSignal("forge returned no open PR".to_string()),
        Err(e) => CiProbe::NoSignal(format!("{e:#}")),
    }
}

// trace:BUG-1037 | ai:codex
pub(crate) fn ci_probe_with_forge(
    project_root: &std::path::Path,
    forge_kind: crate::forge::ForgeKind,
    branch: &str,
) -> CiProbe {
    ci_probe_from_ci_probe_result(
        crate::forge::forge_for_kind(project_root, forge_kind).ci_probe_for_branch(branch),
    )
}

/// STORY-516: forge-routed orchestrator / `--watch-ci` CI watch — blocks until
/// the branch's workflow-run CI is terminal (streaming when interactive),
/// returning CiProbe. GitHubForge delegates to `watch_ci_for_context`.
/// `no_human_active` is the headless flag (inverted to `interactive`).
///
/// TASK-1165: takes the caller's `project_root` instead of re-deriving one from
/// the process cwd. Both callers already hold it, and the old
/// `unwrap_or(PathBuf::from("."))` fallback could hand the forge — and through
/// it the `CiTerminal` emit — a root of `.`, dropping a stray
/// `./.aida/events.jsonl` outside any project.
// trace:STORY-516 trace:TASK-1165 | ai:claude
pub(crate) fn watch_ci_for_context_via_forge(
    project_root: &std::path::Path,
    branch: &str,
    no_human_active: bool,
) -> CiProbe {
    watch_ci_for_context(Some(project_root), branch, no_human_active)
}

// trace:BUG-1037 | ai:codex
pub(crate) fn watch_ci_for_context_with_forge(
    project_root: &std::path::Path,
    forge_kind: crate::forge::ForgeKind,
    branch: &str,
    no_human_active: bool,
) -> CiProbe {
    ci_probe_from_ci_probe_result(
        crate::forge::forge_for_kind(project_root, forge_kind)
            .stream_ci_for_branch(branch, !no_human_active),
    )
}

/// Pure JSON-to-CiProbe parser. Extracted so we can unit-test it without
/// running gh. The input shape is `[{"number": N, "statusCheckRollup": [...]}]`
/// (a JSON array of PR objects from gh; we only ever look at the first).
///
/// BUG-1455: a concluded failure is reported as terminal `Red` only once
/// every other check on the rollup has also concluded. `gh`'s rollup carries
/// no `isRequired` flag, so this function cannot tell a required check from
/// an optional one — but it can always tell "concluded" from "still
/// running", and a check still `IN_PROGRESS`/`QUEUED` might be the one that
/// actually decides code health (e.g. the build), even while a fast-failing
/// gate check (e.g. a supervised merge-hold marker check, which fails by
/// construction) has already concluded. Ending the wait on the gate's
/// conclusion alone let a caller declare CI red — and emit a terminal wake —
/// before the real build had even reported in. Waiting for every check to
/// settle before returning Red or Green is the conservative, always-safe
/// reading: it never reports a verdict while something is still pending or
/// unknown (PRIN-5), at the cost of not fast-failing on an unrelated
/// optional check while something else is still running.
// trace:BUG-1455 | ai:claude
pub(crate) fn parse_ci_probe(stdout: &str) -> CiProbe {
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return CiProbe::NoSignal("no open PR for branch".to_string());
    }
    let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(e) => return CiProbe::NoSignal(format!("gh json parse: {e}")),
    };
    let pr = match parsed.get(0).cloned() {
        Some(v) => v,
        None => return CiProbe::NoSignal("gh returned empty array".to_string()),
    };
    let pr_number = pr.get("number").and_then(|n| n.as_u64()).map(|n| n as u32);
    let pr_number = match pr_number {
        Some(n) if n > 0 => n,
        Some(_) => return CiProbe::NoSignal("gh json PR number was 0".to_string()),
        None => return CiProbe::NoSignal("gh json missing PR number".to_string()),
    };
    let raw_rollup = pr.get("statusCheckRollup").and_then(|v| v.as_array());
    let raw_rollup = match raw_rollup {
        Some(r) if !r.is_empty() => r,
        _ => return CiProbe::PrNoChecks { pr_number },
    };
    // TASK-1424 safety net: the GitLab-mirror-link status is informational
    // only (it always posts `success` — see
    // `gitlab_mirror_link::post_github_mirror_status` — so a real
    // failure/pending mirror entry should be unreachable in practice), but
    // excluding its context here too means a future change to that
    // invariant still can't shelve or stall a drain on GitLab-mirror
    // evidence, which is advisory, not a gate. Filtered out BEFORE the
    // empty-rollup check below (not skipped mid-loop): a PR carrying only
    // the mirror status must read as "no checks yet" (`PrNoChecks`), not as
    // a vacuously passing `Green` from an empty tally. Checked by context
    // (StatusContext shape) or name (CheckRun shape) — whichever the rollup
    // entry carries.
    // trace:TASK-1424 | ai:claude
    let rollup: Vec<&serde_json::Value> = raw_rollup
        .iter()
        .filter(|check| {
            let check_id = check
                .get("context")
                .and_then(|v| v.as_str())
                .or_else(|| check.get("name").and_then(|v| v.as_str()));
            check_id != Some(crate::gitlab_mirror_link::MIRROR_STATUS_CONTEXT)
        })
        .collect();
    if rollup.is_empty() {
        return CiProbe::PrNoChecks { pr_number };
    }
    // Tally check states. statusCheckRollup entries can be from
    // CheckRun (status=COMPLETED|IN_PROGRESS|QUEUED, conclusion=SUCCESS|FAILURE|...)
    // or StatusContext (state=SUCCESS|FAILURE|PENDING|ERROR). Handle both.
    let mut any_in_progress = false;
    let mut failed: Vec<String> = Vec::new();
    for check in rollup {
        // CheckRun shape
        if let Some(status) = check.get("status").and_then(|v| v.as_str()) {
            let conclusion = check
                .get("conclusion")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match status {
                "IN_PROGRESS" | "QUEUED" | "PENDING" | "WAITING" => any_in_progress = true,
                "COMPLETED" => {
                    if matches!(
                        conclusion,
                        "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STALE"
                    ) {
                        let name = check
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("check");
                        failed.push(name.to_string());
                    }
                }
                _ => {}
            }
        } else if let Some(state) = check.get("state").and_then(|v| v.as_str()) {
            // StatusContext shape
            match state {
                "PENDING" | "EXPECTED" => any_in_progress = true,
                "FAILURE" | "ERROR" => {
                    let name = check
                        .get("context")
                        .and_then(|v| v.as_str())
                        .unwrap_or("status");
                    failed.push(name.to_string());
                }
                _ => {}
            }
        }
    }
    // BUG-1455: a check still running always keeps the verdict open, even
    // when another check has already concluded a failure — a partial
    // rollup must never be read as terminal. Only once nothing is left
    // running do concluded failures decide Red vs Green.
    if any_in_progress {
        return CiProbe::InProgress { pr_number };
    }
    if !failed.is_empty() {
        let summary = if failed.len() <= 3 {
            failed.join(", ")
        } else {
            format!("{} and {} more", failed[..3].join(", "), failed.len() - 3)
        };
        return CiProbe::Red {
            pr_number,
            failed_summary: summary,
        };
    }
    CiProbe::Green { pr_number }
}

/// TASK-1453: at the CI wait's absolute ceiling, decide whether a stuck-forever
/// pending check should still be reported as a known `Red` rather than the
/// uninformative `NoSignal` the wait falls back to today. Parses the same
/// `gh pr list --json number,statusCheckRollup,headRefOid` rollup shape
/// `parse_ci_probe` consumes (it's the exact string `ci_progress_snapshot`
/// already fetched this poll for the idle fingerprint — no extra probe).
///
/// Returns `Some(Red)`, naming the concluded failure(s) AND listing the
/// still-pending check(s), only when at least one check has genuinely
/// concluded a failure. A rollup with nothing but pending/queued checks (the
/// honest "we truly don't know yet" case) returns `None` so the caller keeps
/// reporting `NoSignal` — stuck-pending alone must never be read as Red.
/// Pure + unit-tested; degrades to `None` on unparsable/foreign-shaped JSON
/// (e.g. a non-GitHub forge's progress snapshot), which is the safe fallback.
// trace:TASK-1453 | ai:claude
pub(crate) fn ci_ceiling_verdict_from_rollup(rollup_json: &str) -> Option<CiProbe> {
    let trimmed = rollup_json.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let pr = parsed.get(0)?;
    let pr_number = pr
        .get("number")
        .and_then(|n| n.as_u64())
        .filter(|n| *n > 0)? as u32;
    let rollup = pr.get("statusCheckRollup").and_then(|v| v.as_array())?;
    let mut failed: Vec<String> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for check in rollup {
        if let Some(status) = check.get("status").and_then(|v| v.as_str()) {
            let conclusion = check
                .get("conclusion")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = check
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("check");
            match status {
                "IN_PROGRESS" | "QUEUED" | "PENDING" | "WAITING" => pending.push(name.to_string()),
                "COMPLETED" => {
                    if matches!(
                        conclusion,
                        "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STALE"
                    ) {
                        failed.push(name.to_string());
                    }
                }
                _ => {}
            }
        } else if let Some(state) = check.get("state").and_then(|v| v.as_str()) {
            let name = check
                .get("context")
                .and_then(|v| v.as_str())
                .unwrap_or("status");
            match state {
                "PENDING" | "EXPECTED" => pending.push(name.to_string()),
                "FAILURE" | "ERROR" => failed.push(name.to_string()),
                _ => {}
            }
        }
    }
    // Review fix: the merge-hold gate fails BY CONSTRUCTION while a hold is
    // active; it is never a real CI failure. At the ceiling it must not turn a
    // held PR with a stuck check into ci-red. trace:TASK-1453 | ai:claude
    failed.retain(|name| !name.eq_ignore_ascii_case(crate::ci_gate::HOLD_GATE_CHECK));
    if failed.is_empty() {
        return None;
    }
    let failed_summary = if failed.len() <= 3 {
        failed.join(", ")
    } else {
        format!("{} and {} more", failed[..3].join(", "), failed.len() - 3)
    };
    let summary = if pending.is_empty() {
        failed_summary
    } else {
        format!("{failed_summary} (still pending: {})", pending.join(", "))
    };
    Some(CiProbe::Red {
        pr_number,
        failed_summary: summary,
    })
}

/// Block until the branch's CI run reaches a terminal state. Polls every 30s.
///
/// TASK-968: the wait runs an IDLE timeout (re-arms on progress) with a SEPARATE
/// absolute ceiling, instead of the old flat absolute timeout. The idle deadline
/// resets whenever CI posts progress (a check appears/transitions, the check set
/// changes) OR the PR head / base tip advances (the PR got rebased), so a
/// legitimately-slow-but-moving CI keeps its monitor; only a genuine STALL dies
/// after the idle window. The absolute ceiling still bounds a forever-progressing
/// wait so it can't run unbounded. Returns NoSignal on either timeout so the
/// drain gate can shelve rather than hanging or proceeding unchecked. Ctrl+C
/// interrupts cleanly.
/// Windows: `AIDA_WORKER_CI_IDLE` / `[drain] ci_idle` (default 1200s) and
/// `AIDA_WORKER_CI_ABSOLUTE` / `[drain] ci_absolute` (default 5400s).
///
/// TASK-1165: `project_root` is INJECTED by the caller rather than resolved
/// here from the process cwd — the same rule BUG-770 established for the
/// escalation epilogue. Every caller already holds the root it is driving
/// (`session_end`'s `project_root`, `RealPhaseDriver`'s `self.project_root`,
/// `GitHubForge`'s `self.project_root`), so nothing has to guess. `None` skips
/// the terminal `CiTerminal` emit entirely instead of falling back to `.` and
/// dropping a stray `./.aida/events.jsonl` outside any project; the git probes
/// below still degrade to the cwd, which is what they always did.
// trace:TASK-111 trace:TASK-968 trace:TASK-1165 | ai:claude
pub(crate) fn wait_for_ci_terminal(
    project_root: Option<&std::path::Path>,
    branch: &str,
) -> CiProbe {
    // The git probes are read-only and harmless against the cwd; only the emit
    // is root-sensitive, so only the emit is gated on `Some`.
    let git_root = project_root.unwrap_or_else(|| std::path::Path::new("."));
    use crate::ci_idle_timeout::{
        ci_observation_rearms_idle, ci_progress_fingerprint, ci_wait_verdict, CiWaitVerdict,
    };
    const POLL_INTERVAL_SECS: u64 = 30;
    let idle_window = crate::ci_idle_timeout::ci_idle_window_secs(git_root);
    let absolute_ceiling = crate::ci_idle_timeout::ci_absolute_ceiling_secs(git_root);
    let default_ref = detect_default_branch_ref(git_root);

    let started = std::time::Instant::now();
    let mut last_progress = started;
    let mut last_fingerprint: Option<String> = None;
    let retry_config = crate::network_retry::RetryConfig::load(git_root);
    let mut consecutive_probe_failures = 0_u32;

    loop {
        std::thread::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS));
        // Keep every poll on the caller-injected repository. Re-discovering from
        // the process cwd can silently switch a GitLab wait to GitHub when the
        // driven worktree is not the agent's cwd. trace:TASK-1273 | ai:codex
        let forge_kind = crate::forge::resolve_forge_kind(git_root);
        let probe = ci_probe_with_forge(git_root, forge_kind, branch);
        let total_elapsed = started.elapsed().as_secs();

        // Progress detection: re-arm the idle deadline whenever the CI check set
        // transitions/appears OR the PR head / base tip advances (a rebase).
        let base_tip = default_ref
            .as_deref()
            .and_then(|r| git_rev_parse_quiet(git_root, r));
        // STORY-1166: the idle-progress fingerprint source is forge-routed —
        // GitHub check rollup, GitLab pipelines listing; pure-git has none.
        // trace:STORY-1166 | ai:claude
        let rollup = crate::forge::forge_for(git_root).ci_progress_snapshot(branch);
        let fingerprint = ci_progress_fingerprint(&rollup, base_tip.as_deref());
        let progressed = match &last_fingerprint {
            // The first observation is the baseline, not progress.
            Some(prev) => *prev != fingerprint,
            None => false,
        };
        // BUG-1275: a required check that is still queued/running proves the
        // wait is healthy even if GitHub's rollup is byte-for-byte unchanged.
        // Absence/NoSignal does not re-arm, preserving genuine-stall behavior.
        if progressed || ci_observation_rearms_idle(matches!(probe, CiProbe::InProgress { .. })) {
            last_progress = std::time::Instant::now();
        }
        last_fingerprint = Some(fingerprint);
        let idle_elapsed = last_progress.elapsed().as_secs();

        match &probe {
            CiProbe::NoSignal(reason) => {
                consecutive_probe_failures += 1;
                match decide_ci_probe_failure(
                    reason,
                    consecutive_probe_failures,
                    retry_config.max_attempts,
                    &retry_config.transient_patterns,
                ) {
                    CiProbeFailureAction::Retry => {
                        let backoff = retry_config.base_delay.saturating_mul(
                            retry_config
                                .factor
                                .saturating_pow(consecutive_probe_failures - 1),
                        );
                        eprintln!(
                            "  ↻ CI probe unavailable (attempt {}/{}) — retrying in {}ms: {}",
                            consecutive_probe_failures,
                            retry_config.max_attempts,
                            backoff.as_millis(),
                            reason
                        );
                        std::thread::sleep(backoff);
                        continue;
                    }
                    CiProbeFailureAction::Unavailable => return probe,
                }
            }
            CiProbe::InProgress { pr_number } => {
                consecutive_probe_failures = 0;
                match ci_wait_verdict(total_elapsed, idle_elapsed, idle_window, absolute_ceiling) {
                    CiWaitVerdict::Continue => {
                        eprintln!(
                            "  {} CI still running on PR-{} ({}m {}s elapsed, {}s since progress)",
                            crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                            pr_number,
                            total_elapsed / 60,
                            total_elapsed % 60,
                            idle_elapsed,
                        );
                    }
                    CiWaitVerdict::IdleTimeout => {
                        return CiProbe::NoSignal(format!(
                            "CI idle for {idle_window}s with no progress — giving up"
                        ));
                    }
                    CiWaitVerdict::AbsoluteTimeout => {
                        // TASK-1453: a check stuck pending forever must not
                        // swallow a failure another check already concluded —
                        // report the known Red (stuck check listed as still
                        // pending) instead of the uninformative NoSignal.
                        // Stuck-pending alone (no concluded failure) keeps the
                        // honest NoSignal below.
                        if let Some(red) = ci_ceiling_verdict_from_rollup(&rollup) {
                            eprintln!(
                                "  {} CI wait hit absolute ceiling ({}m) with a known failure — reporting Red",
                                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                                absolute_ceiling / 60,
                            );
                            emit_ci_terminal(project_root, false);
                            return red;
                        }
                        return CiProbe::NoSignal(format!(
                            "CI wait hit absolute ceiling ({}m) — giving up",
                            absolute_ceiling / 60
                        ));
                    }
                }
            }
            // Any non-InProgress probe is the terminal CI verdict.
            // STORY-712: emit the actionable CiTerminal wake before returning.
            // Best-effort, no control-flow change. trace:TASK-988 | ai:claude
            _ => {
                emit_ci_terminal(project_root, matches!(probe, CiProbe::Green { .. }));
                return probe;
            }
        }
    }
}

/// STORY-712: emit the actionable `CiTerminal` wake once a CI watch reaches a
/// terminal verdict. Best-effort, no control-flow effect.
///
/// TASK-1165: the root is INJECTED (see [`wait_for_ci_terminal`]). `None` — a
/// caller with no resolvable project — skips the emit rather than writing a
/// stray `./.aida/events.jsonl` into whatever directory the process happened to
/// be standing in. Split out of the poll loop so both arms of that decision are
/// unit-testable without a 30s `gh` poll.
// trace:TASK-1165 | ai:claude
pub(crate) fn emit_ci_terminal(project_root: Option<&std::path::Path>, green: bool) {
    let Some(root) = project_root else {
        return;
    };
    let (spec, run_uuid) = drain_state::current_context(root);
    events::emit(
        root,
        &events::Event::new(spec, run_uuid, events::EventKind::CiTerminal { green }),
    );
}

/// TASK-968: fetch the raw `statusCheckRollup` (+ `headRefOid`) JSON for the
/// branch's open PR, for the idle-timeout progress fingerprint. Mirrors
/// `probe_ci_state_for_branch`'s `gh pr list` call but returns the raw stdout
/// instead of a parsed verdict. Empty string on any failure (gh missing, no PR,
/// network blip) — the fingerprint then degrades to the base tip only, which
/// still re-arms on a rebase.
// trace:TASK-968 | ai:claude
pub(crate) fn ci_rollup_json_for_branch(branch: &str) -> String {
    let Some(gh) = resolve_forge_cli(crate::forge::ForgeKind::GitHub) else {
        return String::new();
    };
    let output = std::process::Command::new(&gh)
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "open",
            "--json",
            "number,statusCheckRollup,headRefOid",
            "--limit",
            "1",
        ])
        .output_retrying_etxtbsy();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => String::new(),
    }
}

/// TASK-968: resolve a ref to its commit SHA via `git rev-parse`, quietly.
/// `None` on any failure — the caller folds it into the progress fingerprint,
/// so a missing base tip just drops that one progress signal.
// trace:TASK-968
pub(crate) fn git_rev_parse_quiet(project_root: &std::path::Path, refname: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            git_arg_guard::END_OF_OPTIONS,
            refname,
        ]) // trace:BUG-1622 | ai:claude
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// BUG-273: `gh run watch` is an interactive terminal renderer. When stdout
/// is piped to tee, or when an auto-complete drain is explicitly headless, its
/// redraw frames become hundreds of repeated log blocks. Stream only for the
/// true interactive case; otherwise use the quiet poller.
/// trace:BUG-273
pub(crate) fn should_stream_ci_watch(stdout_is_tty: bool, no_human_active: bool) -> bool {
    stdout_is_tty && !no_human_active
}

/// TASK-1165: `project_root` is threaded straight through to the emit site in
/// [`wait_for_ci_terminal`] — this layer only picks stream-vs-poll.
// trace:TASK-1165 | ai:claude
pub(crate) fn watch_ci_for_context(
    project_root: Option<&std::path::Path>,
    branch: &str,
    no_human_active: bool,
) -> CiProbe {
    if let Some(project_root) = project_root {
        let forge_kind = crate::forge::resolve_forge_kind(project_root);
        return ci_probe_from_ci_probe_result(
            crate::forge::forge_for_kind(project_root, forge_kind)
                .stream_ci_for_branch(branch, !no_human_active),
        );
    }
    watch_ci_for_context_github(project_root, branch, no_human_active)
}

// STORY-1163: raw GitHub watcher used by GitHubForge after the public helper
// became forge-dispatched. Keeping the old implementation here preserves the
// GitHub stream/poll behavior while allowing GitLab callers to route to glab.
// trace:STORY-1163 | ai:codex
pub(crate) fn watch_ci_for_context_github(
    project_root: Option<&std::path::Path>,
    branch: &str,
    no_human_active: bool,
) -> CiProbe {
    if should_stream_ci_watch(std::io::stdout().is_terminal(), no_human_active) {
        watch_ci_terminal(project_root, branch)
    } else {
        eprintln!(
            "  {} stdout is non-interactive or headless mode is active — using quiet CI polling.",
            crate::glyph(crate::glyphs::Glyph::Info).cyan()
        );
        wait_for_ci_terminal(project_root, branch)
    }
}

/// TASK-233: extract the most-recent workflow run id from `gh run list
/// --json databaseId` JSON output. Pure — unit-testable independent of
/// the `gh` subprocess. trace:TASK-233 | ai:claude
pub(crate) fn first_run_id_from_gh_json(json: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(json).ok()?;
    let id = parsed.as_array()?.first()?.get("databaseId")?;
    id.as_u64().map(|n| n.to_string())
}

#[cfg(test)]
#[path = "tests/task1165_events_root_tests.rs"]
mod task1165_events_root_tests;

/// TASK-233: `--watch-ci` — stream live CI progress via `gh run watch`
/// rather than silently polling, then re-probe for the terminal state so
/// the end-session decision tree (green proceeds / red prompts) runs
/// exactly as it does for `--wait-ci`. Falls back to the silent poll
/// loop when `gh` is missing or no run id resolves. trace:TASK-233
// trace:TASK-1165 | ai:claude
pub(crate) fn watch_ci_terminal(project_root: Option<&std::path::Path>, branch: &str) -> CiProbe {
    let Some(gh) = resolve_forge_cli(crate::forge::ForgeKind::GitHub) else {
        return wait_for_ci_terminal(project_root, branch);
    };
    // Resolve the latest workflow run id for this branch.
    let run_id = std::process::Command::new(&gh)
        .args([
            "run",
            "list",
            "--branch",
            branch,
            "--limit",
            "1",
            "--json",
            "databaseId",
        ])
        .output_retrying_etxtbsy()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| first_run_id_from_gh_json(&String::from_utf8_lossy(&o.stdout)));
    let Some(run_id) = run_id else {
        // No resolvable run — degrade to the silent poll loop rather
        // than failing the session-end.
        return wait_for_ci_terminal(project_root, branch);
    };
    eprintln!(
        "  {} streaming `gh run watch {}`",
        crate::glyph(crate::glyphs::Glyph::FlowActive).cyan(),
        run_id.dimmed()
    );
    // Inherit stdio so gh's live-updating display renders straight to
    // the user's terminal. `gh run watch` returns when the run reaches
    // a terminal state.
    let _ = std::process::Command::new(&gh)
        .args(["run", "watch", &run_id])
        .status_retrying_etxtbsy();
    // Re-probe to classify Green/Red for the end-session decision tree.
    // Keep the re-probe on the caller-injected root. Re-deriving from the
    // process cwd can silently route a GitLab branch through GitHub when the
    // driven worktree is not the agent's cwd. Only the `None` arm — where the
    // caller supplied no root — keeps the historical cwd discovery.
    // trace:TASK-1273 | ai:claude
    let probe_root = project_root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| find_project_root().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    let forge_kind = crate::forge::resolve_forge_kind(&probe_root);
    ci_probe_with_forge(&probe_root, forge_kind, branch) // STORY-516: forge-routed
}

/// Open-PR metadata captured by `gh pr list` for a session's branch. Just
/// the fields the auto-queue side-effect needs to brief a reviewer.
/// trace:STORY-66 | ai:claude
pub(crate) struct OpenPrInfo {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) head_branch: Option<String>,
}

/// Why `detect_open_pr_for_branch` returned no PR — so the caller (and the
/// user reading `aida session end` output) can tell "no PR yet" from "gh
/// isn't installed" from "gh blew up". The old None-everything return type
/// hid the difference and made BUG-72 hard to diagnose.
///
/// BUG-257 split `GhFailed` further: a *transient* `GhUnreachable` (the GH
/// API was unreachable) is distinct from a `GhFailed` (auth, parse, gh
/// itself errored). The orchestrator's phase-1 reports the first as
/// *Inconclusive* (drain pauses, retry later) and the second as a phase
/// failure — same as before. Conflating the two made every network blip
/// look like a "no PR" failure with a misleading recovery hint.
/// trace:BUG-72 BUG-257 | ai:claude
pub(crate) enum PrLookup {
    Found(OpenPrInfo),
    /// `gh` ran cleanly but reported no open PR for this branch.
    NoOpenPr,
    /// `gh` is not on $PATH (binary missing).
    GhMissing,
    /// `gh` was found but its invocation failed (auth, parse, ...). The
    /// String carries the trimmed stderr / parse error so the user sees
    /// the actual cause instead of a silent no-op.
    GhFailed(String),
    /// `gh` ran but could not reach the GitHub API — a *transient* network
    /// error (DNS, TCP, TLS, githubstatus pointer). The orchestrator treats
    /// this as Inconclusive (the API outage means we cannot tell whether a
    /// PR exists), not as a phase failure. trace:BUG-257 | ai:claude
    GhUnreachable(String),
}

/// BUG-1288: the wall-clock ceiling a single forge-CLI subprocess (`gh`/`glab`)
/// may run before [`command_output_with_timeout`] gives up on it. Measured
/// cause of this spec's `aida status --full` / `aida awaiting --json` stall:
/// one `gh api .../branches/main/protection` call took 10.5s in this
/// repository while every other `gh` call in the same run finished in under a
/// second — `gh` itself has no request-timeout flag, so an occasional slow
/// endpoint or rate-limit backoff can otherwise consume the whole machine-
/// readable budget on ONE subprocess.
// trace:BUG-1288 | ai:claude
pub(crate) const FORGE_CLI_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Review follow-up (BUG-1288): on unix, kill the CHILD'S WHOLE PROCESS
/// GROUP rather than only the direct child. `command_output_with_timeout`
/// spawns with `process_group(0)` below, which makes the child's own pid its
/// process group id, so any grandchild it forks (a credential-manager
/// helper, a background `git` op) inherits that same group — `killpg` reaps
/// the group in one signal instead of leaving a grandchild alive to hold the
/// stdout/stderr pipe write ends open past the timeout. `pid` must be a pid
/// this process spawned with `process_group(0)` (the only caller), which is
/// what makes it both safe to signal and a valid process-group id.
// trace:BUG-1288 | ai:claude
#[cfg(unix)]
pub(crate) fn kill_process_group(pid: u32) {
    // SAFETY: `pid` is this process's own child (see the doc comment above),
    // and `libc::killpg` is a plain signal-delivery syscall — no pointers,
    // no aliasing concerns.
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

// trace:TASK-1535 | ai:codex
pub(crate) enum BoundedCommandOutput {
    Completed(std::process::Output),
    SpawnFailed,
    TimedOut,
}

pub(crate) fn command_output_with_timeout_detail(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> BoundedCommandOutput {
    use std::io::Read;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = match cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // BUG-1735: a bare spawn here turns a transient ETXTBSY into
        // `SpawnFailed`, which discards the errno and reads as "could not
        // start". Every bounded-run caller inherits that, so the retry
        // belongs in the wrapper, not at each call site.
        // trace:BUG-1735 | ai:claude
        .spawn_retrying_etxtbsy()
    {
        Ok(child) => child,
        Err(_) => return BoundedCommandOutput::SpawnFailed,
    };
    // Only used to target `killpg` below; on non-unix targets nothing reads
    // it, so it is cfg-gated too rather than left as a dead binding.
    #[cfg(unix)]
    let pid = child.id();
    let mut stdout_pipe = child.stdout.take().expect("stdout was piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        let _ = stdout_tx.send(buf);
    });
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        let _ = stderr_tx.send(buf);
    });
    let start = std::time::Instant::now();
    let mut killed = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    killed = true;
                    #[cfg(unix)]
                    kill_process_group(pid);
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            // BUG-1288 fix-up: a `try_wait` error leaves the child un-reaped
            // exactly like the timeout branch above — kill and wait it here
            // too, or it leaks as an orphan/zombie every time this arm is
            // hit instead of only on the timeout path.
            // trace:BUG-1288 | ai:claude
            Err(_) => {
                killed = true;
                #[cfg(unix)]
                kill_process_group(pid);
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // Bounded on the kill path (short — the group is already dead or dying,
    // this is only a safety net for a descendant that escaped it), generous
    // on the normal-exit path (the child already closed its own pipe ends;
    // this bound exists so a hypothetical stuck reader still can't hang the
    // caller forever, not because it is expected to be hit).
    let read_wait = if killed {
        std::time::Duration::from_millis(500)
    } else {
        std::time::Duration::from_secs(30)
    };
    let stdout = stdout_rx.recv_timeout(read_wait).unwrap_or_default();
    let stderr = stderr_rx.recv_timeout(read_wait).unwrap_or_default();
    match status {
        Some(status) => BoundedCommandOutput::Completed(std::process::Output {
            status,
            stdout,
            stderr,
        }),
        None => BoundedCommandOutput::TimedOut,
    }
}

/// BUG-1594: the message a forge lookup carries when the scoped
/// [`with_forge_lookup_timeout`] ceiling killed it. Rendered by `aida show`
/// as "timed out" rather than a generic unreachable so the output says
/// exactly why the PR/MR state is unknown (PRIN-5).
// trace:BUG-1594 | ai:claude
pub(crate) const FORGE_LOOKUP_TIMED_OUT: &str = "forge lookup timed out";

thread_local! {
    // trace:BUG-1594 | ai:claude
    pub(crate) static FORGE_LOOKUP_TIMEOUT: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

/// BUG-1594: run `f` with every open-change forge lookup it makes (`gh pr
/// list` / `glab api`) bounded by `timeout`. Scoped rather than global so
/// interactive read surfaces (`aida show`) get a hard ceiling while the
/// orchestrator's phase probes keep their existing unbounded semantics.
// trace:BUG-1594 | ai:claude
pub(crate) fn with_forge_lookup_timeout<T>(
    timeout: std::time::Duration,
    f: impl FnOnce() -> T,
) -> T {
    let prev = FORGE_LOOKUP_TIMEOUT.with(|c| c.replace(Some(timeout)));
    struct Restore(Option<std::time::Duration>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0;
            FORGE_LOOKUP_TIMEOUT.with(|c| c.set(prev));
        }
    }
    let _restore = Restore(prev);
    f()
}

/// BUG-1594: spawn a forge-CLI lookup honoring the scoped
/// [`with_forge_lookup_timeout`] ceiling. `Ok(None)` means the call was
/// killed at the ceiling; `Err` is a spawn failure; with no scoped ceiling
/// this is exactly `Command::output()`.
// trace:BUG-1594 | ai:claude
pub(crate) fn forge_lookup_output(
    mut cmd: std::process::Command,
) -> std::io::Result<Option<std::process::Output>> {
    match FORGE_LOOKUP_TIMEOUT.with(|c| c.get()) {
        // trace:BUG-1735 | ai:claude
        None => cmd.output_retrying_etxtbsy().map(Some),
        Some(timeout) => {
            let started = std::time::Instant::now();
            match command_output_with_timeout(cmd, timeout) {
                Some(out) => Ok(Some(out)),
                None if started.elapsed() >= timeout => Ok(None),
                None => Err(std::io::Error::other("could not spawn the forge CLI")),
            }
        }
    }
}

/// Resolve the `glab` (GitLab CLI) binary, mirroring `resolve_gh_binary`'s
/// PATH-walk + sanity-spawn (BUG-74/79). Wired into the forge call sites in
/// follow-on STORY-621 slices. trace:STORY-621 | ai:claude
#[allow(dead_code)] // wired into call sites in follow-on STORY-621 slices
pub(crate) fn resolve_glab_binary() -> Option<std::path::PathBuf> {
    resolve_forge_binary("glab", "AIDA_TEST_GLAB_BINARY", "AIDA_DEBUG_GLAB")
}

#[cfg(test)]
#[path = "tests/forge_binary_resolution_tests.rs"]
mod forge_binary_resolution_tests;

#[cfg(all(test, unix))]
mod story1163_forge_dispatch_tests {
    use super::*;
    use tempfile::TempDir;

    fn gitlab_project() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let aida = tmp.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        std::fs::write(aida.join("config.toml"), "[forge]\nprovider = \"gitlab\"\n").unwrap();
        tmp
    }

    #[test]
    fn detect_open_pr_for_branch_dispatches_gitlab_to_glab() {
        let project = gitlab_project();
        let bin_dir = TempDir::new().unwrap();
        let log = bin_dir.path().join("glab.log");
        let glab = bin_dir.path().join("glab");
        crate::test_exec::write_executable(
            &glab,
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"--version\" ]; then echo glab fake; exit 0; fi\n\
                 printf '%s\\n' \"$*\" >> '{}'\n\
                 printf '%s\\n' '[{{\"iid\":7,\"web_url\":\"https://gitlab.example.com/g/p/-/merge_requests/7\",\"source_branch\":\"feature/x\",\"target_branch\":\"main\",\"title\":\"GitLab MR\"}}]'\n",
                log.display()
            ),
        );
        let _g = crate::test_env::EnvVarGuard::set("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap());

        let found = match detect_open_pr_for_branch(project.path(), "feature/x") {
            PrLookup::Found(pr) => pr,
            _ => panic!("expected GitLab MR lookup"),
        };

        assert_eq!(found.number, 7);
        assert_eq!(found.head_branch.as_deref(), Some("feature/x"));
        let logged = std::fs::read_to_string(log).unwrap();
        assert!(logged.contains("api -X GET projects/:id/merge_requests"));
        assert!(logged.contains("-f source_branch=feature/x"));
        assert!(logged.contains("-f state=opened"));
    }

    #[test]
    fn diff_change_dispatches_gitlab_to_glab() {
        let project = gitlab_project();
        let bin_dir = TempDir::new().unwrap();
        let log = bin_dir.path().join("glab.log");
        let glab = bin_dir.path().join("glab");
        crate::test_exec::write_executable(
            &glab,
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"--version\" ]; then echo glab fake; exit 0; fi\n\
                 printf '%s\\n' \"$*\" >> '{}'\n\
                 exit 0\n",
                log.display()
            ),
        );
        let _g = crate::test_env::EnvVarGuard::set("AIDA_TEST_GLAB_BINARY", glab.to_str().unwrap());

        crate::forge::forge_for_kind(project.path(), crate::forge::ForgeKind::GitLab)
            .diff_change(7)
            .expect("GitLab diff should dispatch through glab");

        let logged = std::fs::read_to_string(log).unwrap();
        assert!(logged.contains("mr diff 7"));
    }
}

/// True when `path` exists as a file and is executable by the current
/// process. On Windows we just check existence (the PATHEXT-aware Rust
/// spawn handles the rest); on Unix we check the executable bit on the
/// metadata. trace:BUG-74 | ai:claude
pub(crate) fn is_executable(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        true
    }
}

/// Format a `gh` spawn failure for a `PrLookup::GhFailed`. The OS reports
/// ENOENT against the *binary* when the spawn's working directory has been
/// removed — even though the binary is perfectly fine. `aida session end`
/// removes the session worktree, so this case is common; say so plainly
/// rather than letting the message read as "gh is missing".
/// trace:BUG-107 | ai:claude
pub(crate) fn gh_spawn_error(
    gh_bin: &std::path::Path,
    cwd: &std::path::Path,
    e: &std::io::Error,
) -> String {
    if !cwd.is_dir() {
        format!(
            "working directory `{}` no longer exists — the `{}` binary is fine \
             (os reports the missing cwd as ENOENT on the spawn): {}",
            cwd.display(),
            gh_bin.display(),
            e
        )
    } else {
        format!(
            "spawn `{}` (cwd `{}`): {}",
            gh_bin.display(),
            cwd.display(),
            e
        )
    }
}

/// Run `gh pr list <filter> --limit 1 --json number,title,url,headRefName` and parse the
/// single result line into a [`PrLookup`]. The shared core behind the
/// branch-keyed and spec-keyed PR lookups — each caller supplies only its
/// distinguishing `<filter>` args (`--head <branch>` / `--search <query>`
/// plus `--state`).
///
/// BUG-74: resolves `gh` via an explicit PATH walk + absolute-path fallback
/// rather than `Command::new("gh")`, because a child Rust process can see a
/// PATH that omits the user's gh install dir. BUG-107: a spawn `NotFound`
/// after the binary resolved means a removed working directory far more
/// often than a missing binary — `gh_spawn_error` names the real culprit.
/// AIDA_DEBUG_GH=1 surfaces the binary search.
/// trace:STORY-66 BUG-72 BUG-74 BUG-107 BUG-223 | ai:claude
pub(crate) fn gh_pr_list_first(project_root: &std::path::Path, filter: &[&str]) -> PrLookup {
    let gh_bin = match resolve_forge_cli(crate::forge::ForgeKind::GitHub) {
        Some(p) => p,
        None => return PrLookup::GhMissing,
    };
    let mut args: Vec<&str> = vec!["pr", "list"];
    args.extend_from_slice(filter);
    args.extend_from_slice(&[
        "--limit",
        "1",
        "--json",
        // trace:BUG-1812 | ai:antigravity
        "number,title,url,headRefName,isDraft",
        "-q",
        r#".[] | "\(.number)\t\(.title)\t\(.url)\t\(.headRefName)""#,
    ]);
    // BUG-1594: honor a caller-scoped ceiling (`aida show`) so one slow
    // `gh` call cannot stall an interactive read. trace:BUG-1594 | ai:claude
    let mut gh_cmd = std::process::Command::new(&gh_bin);
    gh_cmd.current_dir(project_root).args(&args);
    let out = match forge_lookup_output(gh_cmd) {
        Ok(Some(o)) => o,
        Ok(None) => return PrLookup::GhUnreachable(FORGE_LOOKUP_TIMED_OUT.to_string()),
        Err(e) => return PrLookup::GhFailed(gh_spawn_error(&gh_bin, project_root, &e)),
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if gh_stderr_is_network_error(&stderr) {
            return PrLookup::GhUnreachable(if stderr.is_empty() {
                format!("gh exited {}", out.status)
            } else {
                stderr
            });
        }
        return PrLookup::GhFailed(if stderr.is_empty() {
            format!("gh exited {}", out.status)
        } else {
            stderr
        });
    }
    parse_gh_pr_line(&String::from_utf8_lossy(&out.stdout))
}

/// Classify a `gh` stderr line as a transient network error (the GH API
/// was unreachable) rather than an auth/parse/other failure. Used by
/// [`gh_pr_list_first`] to split `PrLookup::GhFailed` into the
/// `GhUnreachable` variant so the orchestrator can report *Inconclusive*
/// instead of conflating "no PR" with "can't reach the API".
///
/// Patterns are taken from real-world `gh` output (the Go HTTP client +
/// gh's own diagnostic suffixes). The match is conservative —
/// case-insensitive substring matching against a small allow-list — so an
/// auth/parse failure never gets re-classified as a transient network
/// blip. trace:BUG-257 | ai:claude
pub(crate) fn gh_stderr_is_network_error(stderr: &str) -> bool {
    // external-prose-classifier: gh_stderr_is_network_error
    aida_core::external_tool_output::contains_any_case_insensitive(
        stderr,
        aida_core::external_tool_output::GH_NETWORK_TRANSIENT,
    )
}

/// STORY-516: reverse of `change_lookup_from_pr_lookup` — convert a forge
/// `ChangeLookup` back to the orchestrator's `PrLookup` so the `*_via_forge`
/// helpers are a pure name-swap at their call sites (match arms stay PrLookup).
/// trace:STORY-516 | ai:claude
pub(crate) fn pr_lookup_from_change_lookup(c: crate::forge::ChangeLookup) -> PrLookup {
    match c {
        crate::forge::ChangeLookup::Found(r) => PrLookup::Found(OpenPrInfo {
            number: r.id,
            title: r.title.unwrap_or_default(),
            url: r.url,
            head_branch: (!r.branch.is_empty()).then_some(r.branch),
        }),
        crate::forge::ChangeLookup::NoChange => PrLookup::NoOpenPr,
        crate::forge::ChangeLookup::CliMissing => PrLookup::GhMissing,
        crate::forge::ChangeLookup::CliFailed(s) => PrLookup::GhFailed(s),
        crate::forge::ChangeLookup::Unreachable(s) => PrLookup::GhUnreachable(s),
    }
}

// STORY-1163: inverse adapter for legacy callers that still consume
// `ChangeLookup` while the central PR helper now dispatches by forge.
// trace:STORY-1163 | ai:codex
pub(crate) fn change_lookup_from_pr_lookup_for_branch(
    p: PrLookup,
    branch_hint: &str,
) -> crate::forge::ChangeLookup {
    match p {
        PrLookup::Found(pr) => crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
            id: pr.number,
            url: pr.url,
            branch: pr
                .head_branch
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| branch_hint.to_string()),
            base: String::new(),
            title: Some(pr.title),
        }),
        PrLookup::NoOpenPr => crate::forge::ChangeLookup::NoChange,
        PrLookup::GhMissing => crate::forge::ChangeLookup::CliMissing,
        PrLookup::GhFailed(s) => crate::forge::ChangeLookup::CliFailed(s),
        PrLookup::GhUnreachable(s) => crate::forge::ChangeLookup::Unreachable(s),
    }
}

/// STORY-516: forge-routed spec-search open-PR lookup (BUG-223 fallback). GitHub
/// delegates to `detect_open_pr_for_spec`. trace:STORY-516 | ai:claude
pub(crate) fn detect_open_pr_for_spec_via_forge(
    project_root: &std::path::Path,
    spec: &str,
) -> PrLookup {
    detect_open_pr_for_spec(project_root, spec)
}

/// STORY-516: forge-routed merged-PR-for-branch lookup. GitHub delegates to
/// `detect_merged_pr_for_branch`. trace:STORY-516 | ai:claude
pub(crate) fn detect_merged_pr_for_branch_via_forge(
    project_root: &std::path::Path,
    branch: &str,
) -> PrLookup {
    detect_merged_pr_for_branch(project_root, branch)
}

/// STORY-516: forge-routed branch lookup — the view-op entry point the call
/// sites use instead of `detect_open_pr_for_branch` directly, so a GitLab /
/// pure-git repo goes through its own provider. GitHubForge delegates back to
/// `detect_open_pr_for_branch`, so behaviour on GitHub is unchanged. The
/// provider returns `Result<ChangeLookup>`; GitHub never errs (it adapts a
/// PrLookup), and a provider Err (GitLab stub) collapses to `CliFailed` so
/// callers keep a single 5-state match. trace:STORY-516 | ai:claude
pub(crate) fn change_lookup_for_branch(
    project_root: &std::path::Path,
    branch: &str,
) -> crate::forge::ChangeLookup {
    change_lookup_from_pr_lookup_for_branch(detect_open_pr_for_branch(project_root, branch), branch)
}

// trace:BUG-876 | ai:codex
pub(crate) fn change_lookup_for_spec(
    project_root: &std::path::Path,
    spec: &str,
) -> crate::forge::ChangeLookup {
    crate::forge::forge_for(project_root)
        .change_for_spec(spec)
        .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}")))
}

/// Look up a single open PR keyed on `branch`. The richer [`PrLookup`]
/// return shape lets callers print an honest skip reason (no PR yet / gh
/// missing / gh failed) instead of collapsing every case to `None`.
/// trace:STORY-66 BUG-72 BUG-74 | ai:claude
pub(crate) fn detect_open_pr_for_branch(project_root: &std::path::Path, branch: &str) -> PrLookup {
    let forge_kind = crate::forge::resolve_forge_kind(project_root);
    pr_lookup_from_change_lookup(
        crate::forge::forge_for_kind(project_root, forge_kind)
            .change_for_branch(branch)
            .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))),
    )
}

// STORY-1163: raw GitHub lookup used by GitHubForge after the public helper
// became forge-dispatched. The `gh pr list` argv/parsing stay unchanged.
// trace:STORY-1163 | ai:codex
pub(crate) fn detect_open_pr_for_branch_github(
    project_root: &std::path::Path,
    branch: &str,
) -> PrLookup {
    gh_pr_list_first(project_root, &["--head", branch, "--state", "open"])
}

/// BUG-257: the three states `probe_branch_on_origin` can settle on when the
/// orchestrator narrows a `PrLookup::GhUnreachable` with `git ls-remote`. The
/// existing `aida_core::git_ops::remote_branch_exists` collapses `Absent` and
/// `LsRemoteFailed` into one `false` — exactly the conflation BUG-257 must
/// avoid here, so we keep this enum and the probe local to the orchestrator
/// caller. trace:BUG-257 | ai:claude
pub(crate) enum BranchOriginProbe {
    /// `git ls-remote` reported the branch is on origin — a PR may exist.
    Present,
    /// `git ls-remote` ran cleanly and reported the branch is absent — no
    /// PR can exist regardless of GH-API state.
    Absent,
    /// `git ls-remote` itself failed (network down end-to-end, auth, ...).
    /// Cannot narrow the diagnosis at all.
    LsRemoteFailed,
}

/// BUG-257: narrow a `PrLookup::GhUnreachable` outcome with `git ls-remote`
/// against `origin`. The git protocol is HTTPS-API-independent, so when the
/// GH API is down `ls-remote` often still works — and a missing branch
/// proves no PR can exist (the implementer never pushed). When the branch
/// IS present we still cannot tell whether a PR was opened against it
/// without the API; that case becomes Inconclusive. When `ls-remote` also
/// fails, the orchestrator has no way to tell and stays Inconclusive.
/// Best-effort: short timeout so a hung connection never blocks phase 1.
/// trace:BUG-257 | ai:claude
pub(crate) fn probe_branch_on_origin(
    project_root: &std::path::Path,
    branch: &str,
) -> BranchOriginProbe {
    let out = std::process::Command::new("git")
        .current_dir(project_root)
        .args([
            "-c",
            // Keep the probe bounded — a hung HTTPS dial against origin
            // would otherwise re-introduce the freeze BUG-257 is fixing.
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=10",
            "ls-remote",
            "--exit-code",
            "--heads",
            "origin",
            &format!("refs/heads/{branch}"),
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => BranchOriginProbe::Present,
        // `git ls-remote --exit-code` exits 2 when the ref is missing on a
        // remote it could otherwise reach. Distinguish that from a true
        // ls-remote failure (network down, auth) by checking the exit code.
        Ok(o) => match o.status.code() {
            Some(2) => BranchOriginProbe::Absent,
            _ => BranchOriginProbe::LsRemoteFailed,
        },
        Err(_) => BranchOriginProbe::LsRemoteFailed,
    }
}

pub(crate) fn spec_branch_slug(spec: &str) -> String {
    spec.trim().to_ascii_lowercase().replace([' ', '_'], "-")
}

pub(crate) fn branch_name_references_spec(branch: &str, spec: &str) -> bool {
    let slug = spec_branch_slug(spec);
    let branch = branch
        .trim()
        .trim_start_matches("origin/")
        .to_ascii_lowercase();
    if slug.is_empty() {
        return false;
    }
    // BUG-1525 review fix: the slug must be a whole segment of the branch
    // name. A raw substring let BUG-15 match `bug-150-work` and BUG-152
    // match `claude/bug-1525`. A boundary is start/end or one of `/-._`,
    // and the character after the slug must not be a digit or letter.
    // trace:BUG-1525 | ai:claude
    let is_sep = |c: char| matches!(c, '/' | '-' | '.' | '_');
    branch.match_indices(&slug).any(|(at, m)| {
        let before_ok = branch[..at].chars().next_back().is_none_or(is_sep);
        let after_ok = branch[at + m.len()..].chars().next().is_none_or(is_sep);
        before_ok && after_ok
    })
}

#[cfg(test)]
mod bug_1525_branch_match_tests {
    use super::branch_name_references_spec as m;

    // trace:BUG-1525 | ai:claude
    #[test]
    fn branch_match_is_whole_segment_only() {
        assert!(m("bug-15-work", "BUG-15"));
        assert!(m("claude/bug-15", "BUG-15"));
        assert!(m("origin/bug-15", "BUG-15"));
        assert!(m("bug-15", "BUG-15"));
        assert!(!m("bug-150-work", "BUG-15"));
        assert!(!m("claude/bug-1525", "BUG-152"));
        assert!(!m("xbug-15", "BUG-15"));
    }
}

pub(crate) fn open_pr_commit_headlines_reference_spec(
    project_root: &std::path::Path,
    pr: u64,
    spec: &str,
) -> bool {
    let Some(gh_bin) = resolve_forge_cli(crate::forge::ForgeKind::GitHub) else {
        return false;
    };
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "view",
            &pr.to_string(),
            "--json",
            "commits",
            "-q",
            ".commits[].messageHeadline",
        ])
        .output_retrying_etxtbsy();
    let Ok(out) = out else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let wanted = spec.to_ascii_uppercase();
    String::from_utf8_lossy(&out.stdout).lines().any(|line| {
        extract_spec_ids_from_commit(line)
            .into_iter()
            .any(|id| id.eq_ignore_ascii_case(&wanted))
    })
}

pub(crate) fn detect_open_pr_for_spec_by_head_or_commit(
    project_root: &std::path::Path,
    spec: &str,
) -> PrLookup {
    let gh_bin = match resolve_forge_cli(crate::forge::ForgeKind::GitHub) {
        Some(p) => p,
        None => return PrLookup::GhMissing,
    };
    let spawned = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            "number,title,url,headRefName,isDraft",
            "-q",
            r#".[] | "\(.number)\t\(.title)\t\(.url)\t\(.headRefName)""#,
        ])
        .output_retrying_etxtbsy();
    let out = match spawned {
        Ok(o) => o,
        Err(e) => return PrLookup::GhFailed(gh_spawn_error(&gh_bin, project_root, &e)),
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if gh_stderr_is_network_error(&stderr) {
            return PrLookup::GhUnreachable(if stderr.is_empty() {
                format!("gh exited {}", out.status)
            } else {
                stderr
            });
        }
        return PrLookup::GhFailed(if stderr.is_empty() {
            format!("gh exited {}", out.status)
        } else {
            stderr
        });
    }

    let mut candidates: Vec<OpenPrInfo> = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let PrLookup::Found(pr) = parse_gh_pr_line(line) {
            candidates.push(pr);
        }
    }

    for pr in &candidates {
        if pr
            .head_branch
            .as_deref()
            .is_some_and(|b| branch_name_references_spec(b, spec))
        {
            return PrLookup::Found(OpenPrInfo {
                number: pr.number,
                title: pr.title.clone(),
                url: pr.url.clone(),
                head_branch: pr.head_branch.clone(),
            });
        }
    }
    for pr in candidates {
        if open_pr_commit_headlines_reference_spec(project_root, pr.number, spec) {
            return PrLookup::Found(pr);
        }
    }
    PrLookup::NoOpenPr
}

/// Fallback PR lookup for BUG-223/BUG-876: find an open PR that references
/// `spec`, used when branch/lease linkage is unavailable or stale. Search title
/// / body first, then open PR head branches (`story-818`) and commit headlines
/// carrying `(SPEC-ID)` trailers so releasing a lease cannot hide a pushed PR.
/// trace:BUG-223 | ai:claude
// trace:BUG-876 | ai:codex
pub(crate) fn detect_open_pr_for_spec(project_root: &std::path::Path, spec: &str) -> PrLookup {
    let forge_kind = crate::forge::resolve_forge_kind(project_root);
    pr_lookup_from_change_lookup(
        crate::forge::forge_for_kind(project_root, forge_kind)
            .change_for_spec(spec)
            .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))),
    )
}

// STORY-1163: raw GitHub spec lookup used by GitHubForge after the public helper
// became forge-dispatched. trace:STORY-1163 | ai:codex
pub(crate) fn detect_open_pr_for_spec_github(
    project_root: &std::path::Path,
    spec: &str,
) -> PrLookup {
    match gh_pr_list_first(project_root, &["--search", spec, "--state", "open"]) {
        PrLookup::NoOpenPr => detect_open_pr_for_spec_by_head_or_commit(project_root, spec),
        other => other,
    }
}

/// TASK-843: list ALL open PRs that reference `spec` (number + head branch),
/// not just the first. `detect_open_pr_for_spec_via_forge` returns at most one,
/// so a spec with >1 open PR (a reopened/duplicate) was resolved by a silent,
/// non-deterministic pick. This surfaces the full candidate set so the pure
/// [`integrate::select_canonical_pr`] policy can pick the newest-canonical PR
/// and report the rest. Best-effort: an empty vec when gh is missing/failing or
/// no PR references the spec — the caller falls back to the single-PR path.
/// trace:TASK-843 | ai:claude
pub(crate) fn all_open_prs_for_spec_via_forge(
    project_root: &std::path::Path,
    spec: &str,
) -> Vec<(u64, String)> {
    let gh_bin = match resolve_gh_binary() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "list",
            "--search",
            spec,
            "--state",
            "open",
            "--limit",
            "20",
            "--json",
            "number,headRefName",
            "-q",
            r#".[] | "\(.number)\t\(.headRefName)""#,
        ])
        .output_retrying_etxtbsy();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (num, branch) = line.split_once('\t')?;
            let number = num.trim().parse::<u64>().ok()?;
            Some((number, branch.trim().to_string()))
        })
        .collect()
}

/// Look up the head branch of an open PR by number — used by the BUG-223
/// spec-id fallback to realign the orchestrator's branch after a swap the
/// worktree-HEAD reconciliation missed, so the CI / merge phases probe the
/// PR's actual head. Best-effort: `None` on any `gh` failure.
/// trace:BUG-223 | ai:claude
pub(crate) fn pr_head_branch(project_root: &std::path::Path, pr_number: u64) -> Option<String> {
    let gh_bin = resolve_gh_binary()?;
    let out = std::process::Command::new(&gh_bin)
        .current_dir(project_root)
        .args([
            "pr",
            "view",
            &pr_number.to_string(),
            "--json",
            "headRefName",
            "-q",
            ".headRefName",
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

/// TASK-1230: the UNIQUE registered worktree checked out on `branch`, excluding
/// the main checkout and the store worktree. Returns None on zero or ambiguous
/// (>1) matches — the caller must never guess which of several to remove.
// trace:TASK-1230 | ai:claude
pub(crate) fn unique_worktree_on_branch(
    project_root: &std::path::Path,
    branch: &str,
    main_root: &std::path::Path,
    store_wt: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if branch.is_empty() {
        return None;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Porcelain blocks are blank-line-separated; each has a `worktree <path>`
    // line and (when on a branch) a `branch refs/heads/<name>` line.
    let mut matches: Vec<std::path::PathBuf> = Vec::new();
    let mut cur_path: Option<std::path::PathBuf> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            cur_path = Some(std::path::PathBuf::from(p.trim()));
        } else if let Some(b) = line.strip_prefix("branch ") {
            let short = b.trim().strip_prefix("refs/heads/").unwrap_or(b.trim());
            if short == branch {
                if let Some(p) = &cur_path {
                    let is_protected = p == main_root
                        || p == store_wt
                        || p.canonicalize().ok() == main_root.canonicalize().ok()
                        || p.canonicalize().ok() == store_wt.canonicalize().ok();
                    if !is_protected {
                        matches.push(p.clone());
                    }
                }
            }
        }
    }
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

/// TASK-1230: count of commits on the worktree's HEAD that are NOT yet on the
/// pushed PR branch (`origin/<branch>`) — genuinely-at-risk unpushed work. Zero
/// means every local commit is safely on the remote PR branch. On any git
/// failure returns a conservative 1 (treat as at-risk → keep).
// trace:TASK-1230 | ai:claude
pub(crate) fn local_commits_not_on_branch(worktree: &std::path::Path, branch: &str) -> u32 {
    if branch.is_empty() {
        return 1;
    }
    let remote_ref = format!("origin/{branch}");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["rev-list", "--count", "HEAD", "--not", &remote_ref])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<u32>()
            .unwrap_or(1),
        // The remote ref not existing, or any other git error, is treated as
        // "cannot prove the work is pushed" → keep (conservative).
        _ => 1,
    }
}

/// Detect whether `branch` was the head of a now-MERGED PR. Used by
/// `aida push` (BUG-88) to warn that new commits will be stranded —
/// pushing to a merged-PR branch puts the commit on `origin/<branch>`
/// but it won't reach `main` without a new PR. Mirrors the shape of
/// [`detect_open_pr_for_branch`] but queries `--state merged` and returns
/// only the first hit (most recent). trace:BUG-88 | ai:claude
pub(crate) fn detect_merged_pr_for_branch(
    project_root: &std::path::Path,
    branch: &str,
) -> PrLookup {
    let forge_kind = crate::forge::resolve_forge_kind(project_root);
    pr_lookup_from_change_lookup(
        crate::forge::forge_for_kind(project_root, forge_kind)
            .merged_change_for_branch(branch)
            .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))),
    )
}

// STORY-1163: raw GitHub merged lookup used by GitHubForge after the public
// helper became forge-dispatched. trace:STORY-1163 | ai:codex
pub(crate) fn detect_merged_pr_for_branch_github(
    project_root: &std::path::Path,
    branch: &str,
) -> PrLookup {
    gh_pr_list_first(project_root, &["--head", branch, "--state", "merged"])
}

/// Is PR #`pr` merged on GitHub? Ground truth for the BUG-241 reconcile step
/// — a reviewer that escalated the merge to a human who merged out-of-band
/// leaves the PR merged but the orchestrator's verdict file absent. `None` on
/// any `gh` failure (binary missing, auth, network): the caller treats
/// "cannot confirm" as "the failure stands", never as a silent success.
/// trace:BUG-241 | ai:claude
/// BUG-286: same intent as the pre-BUG-286 `pr_is_merged` but with a
/// caller-supplied retry sink, so the orchestrator can surface a sub-second
/// transient blip to the drain-state file rather than treating it as
/// "cannot confirm." Non-orchestrator callers (none today) can pass
/// [`network_retry::NoopSink`] to keep the original silent best-effort
/// semantics. trace:BUG-286 | ai:claude
pub(crate) fn pr_is_merged_with_sink(
    project_root: &std::path::Path,
    pr: u32,
    sink: &mut dyn network_retry::RetrySink,
) -> Option<bool> {
    // STORY-621 Slice 2: routed through Forge::change_metadata so the
    // reconcile works on GitLab. GitHubForge keeps the resolve_gh_binary +
    // BUG-286 retry-with-sink behaviour this site had inline; any provider
    // error (CLI missing / failed / unreachable) maps to None — "cannot
    // confirm", never a silent success. trace:TASK-963 | ai:claude
    crate::forge::forge_for(project_root)
        .change_metadata(pr as u64, sink)
        .ok()
        .map(|m| m.state == crate::forge::ChangeState::Merged)
}

/// BUG-245: which SPEC-ID does the PR's commits actually credit?
///
/// Reads the PR's commit subjects via `gh pr view <N> --json commits` and
/// parses `(SPEC-ID)` trailers out of each. Returns:
///   - `Some(dispatched)` when the dispatched id appears among the credits —
///     the common path. No mismatch.
///   - `Some(other)` when the dispatched id is *absent* and a different id is
///     credited. The orchestrator reports this as a `shipped-mismatch`
///     anomaly and credits the truth.
///   - `None` when the PR cannot be inspected, no commit names any spec, or
///     `gh` is unreachable — "cannot determine" resolves to "trust the
///     dispatched id", so the reconcile only ratifies a real mismatch.
///
/// Picks the first credited id when multiple non-dispatched ids appear. The
/// observed BUG-245 case is one commit / one trailer; a PR carrying multiple
/// genuinely-different specs is an open-ended case the operator must triage.
/// trace:BUG-245 | ai:claude
/// BUG-286: same intent as the pre-BUG-286 `pr_credited_spec_id` but with a
/// caller-supplied retry sink. The orchestrator passes a [`crate::network_retry::DualSink`]
/// (stderr + drain-state); a hypothetical silent caller can pass
/// [`network_retry::NoopSink`]. trace:BUG-286 | ai:claude
pub(crate) fn pr_credited_spec_id_with_sink(
    project_root: &std::path::Path,
    pr: u32,
    dispatched: &str,
    sink: &mut dyn network_retry::RetrySink,
) -> Option<String> {
    // STORY-621 Slice 2: routed through Forge::change_commit_headlines
    // (GitHubForge issues the same `gh pr view --json commits` argv this site
    // had inline, retry-wrapped). trace:TASK-963 | ai:claude
    let headlines = crate::forge::forge_for(project_root)
        .change_commit_headlines(pr as u64, sink)
        .ok()?;
    pick_credited_spec(&headlines.join("\n"), dispatched)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrCreditMatch {
    Dispatched,
    Other(String),
    Unknown,
}

/// BUG-357: classify whether PR metadata credits the dispatched spec before a
/// reconcile step treats a merged PR as out-of-band success. Commit trailers
/// are strongest; a different commit trailer is an explicit mismatch. PR title
/// trailers are accepted when commits carry no SPEC-ID, matching `aida pr ship`
/// and manual squash flows where the PR title is the only durable trailer.
pub(crate) fn classify_pr_credit(
    commit_subjects: &str,
    pr_title: Option<&str>,
    dispatched: &str,
) -> PrCreditMatch {
    match pick_credited_spec(commit_subjects, dispatched) {
        Some(id) if id.eq_ignore_ascii_case(dispatched) => return PrCreditMatch::Dispatched,
        Some(id) => return PrCreditMatch::Other(id),
        None => {}
    }

    if let Some(title) = pr_title {
        let mut first_other: Option<String> = None;
        for id in extract_spec_ids_from_commit(title) {
            if id.eq_ignore_ascii_case(dispatched) {
                return PrCreditMatch::Dispatched;
            }
            if first_other.is_none() {
                first_other = Some(id);
            }
        }
        if let Some(id) = first_other {
            return PrCreditMatch::Other(id);
        }
    }

    PrCreditMatch::Unknown
}

pub(crate) fn pr_credit_match_with_sink(
    project_root: &std::path::Path,
    pr: u32,
    dispatched: &str,
    pr_title: Option<&str>,
    sink: &mut dyn network_retry::RetrySink,
) -> PrCreditMatch {
    let initial_title_match = pr_title
        .map(|title| classify_pr_credit("", Some(title), dispatched))
        .unwrap_or(PrCreditMatch::Unknown);
    if matches!(initial_title_match, PrCreditMatch::Dispatched) {
        return PrCreditMatch::Dispatched;
    }

    // STORY-621 Slice 2: routed through the forge metadata reads (BUG-357
    // reconcile works on GitLab). Commit headlines settle most cases; the PR
    // title (Forge::change_metadata) is fetched only when the commits leave
    // the credit Unknown — same precedence as the single gh call this
    // replaces, paying for the scalar read only when it is load-bearing.
    // trace:TASK-963 | ai:claude
    let forge = crate::forge::forge_for(project_root);
    let subjects = match forge.change_commit_headlines(pr as u64, sink) {
        Ok(headlines) => headlines.join("\n"),
        Err(_) => return PrCreditMatch::Unknown,
    };
    match classify_pr_credit(&subjects, None, dispatched) {
        PrCreditMatch::Dispatched => PrCreditMatch::Dispatched,
        PrCreditMatch::Other(id) => PrCreditMatch::Other(id),
        PrCreditMatch::Unknown => {
            let fetched_title = forge
                .change_metadata(pr as u64, sink)
                .ok()
                .map(|m| m.title)
                .filter(|t| !t.trim().is_empty());
            match classify_pr_credit("", fetched_title.as_deref(), dispatched) {
                PrCreditMatch::Unknown => initial_title_match,
                title_match => title_match,
            }
        }
    }
}

/// Pure selector for the credited spec id, given newline-separated commit
/// subjects (one per commit on the PR) and the dispatched id. Split out so
/// the precedence rules are unit-testable without spawning `gh`.
/// trace:BUG-245 | ai:claude
pub(crate) fn pick_credited_spec(commit_subjects: &str, dispatched: &str) -> Option<String> {
    let mut first_other: Option<String> = None;
    for subject in commit_subjects.lines() {
        for id in extract_spec_ids_from_commit(subject) {
            if id == dispatched {
                return Some(id);
            }
            if first_other.is_none() {
                first_other = Some(id);
            }
        }
    }
    first_other
}

/// Parse a single tab-separated `gh pr list -q ...` line into a
/// `PrLookup`. Extracted for unit-testing the empty / well-formed /
/// malformed shapes without spawning `gh`. trace:BUG-88 | ai:claude
pub(crate) fn parse_gh_pr_line(stdout: &str) -> PrLookup {
    let Some(line) = stdout.lines().next().map(str::trim) else {
        return PrLookup::NoOpenPr;
    };
    if line.is_empty() {
        return PrLookup::NoOpenPr;
    }
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 3 {
        return PrLookup::GhFailed(format!("could not parse gh output: {:?}", line));
    }
    let Ok(number) = parts[0].parse::<u64>() else {
        return PrLookup::GhFailed(format!("non-numeric PR number from gh: {:?}", parts[0]));
    };
    PrLookup::Found(OpenPrInfo {
        number,
        title: parts[1].to_string(),
        url: parts[2].to_string(),
        head_branch: parts
            .get(3)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    })
}

pub(crate) fn open_pr_review_story_using(
    project_root: &std::path::Path,
    pr_number: u64,
    aida: &std::path::Path,
) -> Option<String> {
    let Ok(out) = std::process::Command::new(aida)
        .current_dir(project_root)
        .args(["list", "--type", "story", "--format", "json"])
        .output_retrying_etxtbsy()
    else {
        return None;
    };
    if !out.status.success() {
        return None;
    }
    open_review_story_id(&String::from_utf8_lossy(&out.stdout), pr_number)
}

/// BUG-1186: does `aida list --type story --format json` output carry a
/// `Review PR-<n>:` story whose round is still open (not Done / Completed /
/// Rejected / Superseded)? Pure so the round semantics are unit-testable.
// trace:BUG-1186 | ai:claude
#[cfg(test)]
pub(crate) fn review_story_round_is_open(list_json: &str, pr_number: u64) -> bool {
    open_review_story_id(list_json, pr_number).is_some()
}

pub(crate) fn open_review_story_id(list_json: &str, pr_number: u64) -> Option<String> {
    let needle = format!("review pr-{}:", pr_number);
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(list_json) else {
        return None;
    };
    rows.iter().find_map(|row| {
        let title = row
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim_start()
            .to_ascii_lowercase();
        if !title.starts_with(&needle) {
            return None;
        }
        let status = row
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-', '_'], "");
        (!matches!(
            status.as_str(),
            "done" | "completed" | "rejected" | "superseded" | "released"
        ))
        .then(|| row.get("spec_id")?.as_str().map(str::to_owned))
        .flatten()
    })
}

pub(crate) fn reviewer_queue_story_ids(project_root: &std::path::Path) -> Option<Vec<String>> {
    let out = std::process::Command::new(aida_exe_path())
        .current_dir(project_root)
        .args([
            "queue",
            "list",
            "--all",
            "--for",
            "reviewer",
            "--json",
            "--no-scope",
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_reviewer_queue_story_ids(&out.stdout)
}

// TASK-192: keep malformed output distinguishable from a valid empty queue so
// callers that gate operator action can fail closed.
// trace:TASK-192 | ai:codex
pub(crate) fn parse_reviewer_queue_story_ids(bytes: &[u8]) -> Option<Vec<String>> {
    let rows = serde_json::from_slice::<Vec<serde_json::Value>>(bytes).ok()?;
    Some(
        rows.iter()
            .filter(|row| row.get("for_role").and_then(|v| v.as_str()) == Some("reviewer"))
            .filter_map(|row| {
                row.get("spec_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            })
            .collect(),
    )
}

// trace:TASK-192 | ai:codex
pub(crate) fn pr_has_merge_hold(
    project_root: &std::path::Path,
    pr: &status_cleanup::OpenPrItem,
) -> bool {
    merge_hold::read_hold(project_root, pr.number).is_some()
        || pr
            .labels
            .iter()
            .any(|label| label.eq_ignore_ascii_case("aida:merge-hold"))
}

// trace:TASK-192 | ai:codex
pub(crate) fn reviewer_route_for_pr(
    routed_prs: Option<&std::collections::HashSet<u64>>,
    pr_number: u64,
) -> awaiting_you::ReviewerRoute {
    match routed_prs {
        Some(prs) if prs.contains(&pr_number) => awaiting_you::ReviewerRoute::Routed,
        Some(_) => awaiting_you::ReviewerRoute::Unrouted,
        None => awaiting_you::ReviewerRoute::Unknown,
    }
}

#[cfg(test)]
mod task_192_fail_closed_fact_tests {
    use super::*;

    fn pr(labels: &[&str]) -> status_cleanup::OpenPrItem {
        status_cleanup::OpenPrItem {
            number: 2035,
            is_draft: false,
            title: "broken".into(),
            head_branch: "broken-pr".into(),
            ci_rollup: Some("fail".into()),
            mergeable: Some("MERGEABLE".into()),
            review_decision: None,
            head_sha: Some("deadbeef".into()),
            labels: labels.iter().map(|label| (*label).into()).collect(),
            created_at: None,
        }
    }

    #[test]
    fn forge_label_only_merge_hold_is_authoritative() {
        let root = tempfile::tempdir().unwrap();
        assert!(pr_has_merge_hold(root.path(), &pr(&["aida:merge-hold"])));
        assert!(pr_has_merge_hold(root.path(), &pr(&["AIDA:MERGE-HOLD"])));
        assert!(!pr_has_merge_hold(root.path(), &pr(&["bug"])));
    }

    #[test]
    fn reviewer_queue_parser_distinguishes_empty_from_malformed() {
        assert_eq!(parse_reviewer_queue_story_ids(b"[]"), Some(Vec::new()));
        assert_eq!(parse_reviewer_queue_story_ids(b"not json"), None);
        assert_eq!(parse_reviewer_queue_story_ids(b"{}"), None);
    }

    #[test]
    fn unavailable_queue_maps_to_unknown_not_unrouted() {
        assert_eq!(
            reviewer_route_for_pr(None, 2035),
            awaiting_you::ReviewerRoute::Unknown
        );
        let available_empty = std::collections::HashSet::new();
        assert_eq!(
            reviewer_route_for_pr(Some(&available_empty), 2035),
            awaiting_you::ReviewerRoute::Unrouted
        );
    }

    #[test]
    fn forge_snapshot_retains_merge_hold_label() {
        let snapshot = parse_open_pr_snapshot(
            r#"[{"number":2035,"title":"broken","headRefName":"broken-pr","labels":[{"name":"aida:merge-hold"}],"statusCheckRollup":[]}]"#,
            None,
        );
        assert_eq!(
            snapshot.by_branch["broken-pr"].labels,
            vec!["aida:merge-hold"]
        );
    }
}

/// Complete inventory for the review-story creation gate only. Bulk readers
/// intentionally tolerate bad objects; absence here must not use that contract.
// trace:BUG-1807 | ai:codex
pub(crate) fn load_review_story_inventory(
    project_root: &std::path::Path,
) -> Result<RequirementsStore> {
    if let Some(store_path) = detect_distributed_store_from(project_root) {
        let objects = store_path.join("objects");
        let mut store = aida_core::GitBackend::read_metadata_only(&store_path)?;
        store.requirements = aida_core::object_store::list_objects_strict(&objects)?
            .into_iter()
            .map(|(_, path)| aida_core::object_store::read_object_from_path(&path))
            .collect::<Result<Vec<_>>>()?;
        return Ok(store);
    }

    // The normal resolver is best effort. Before allowing legacy resolution,
    // distinguish absent config from unreadable config or unavailable storage.
    for root in project_root.ancestors() {
        if aida_core::store_locate::is_system_temp_dir(root) {
            break;
        }
        let path = root.join(".aida/config.toml");
        match std::fs::read_to_string(&path) {
            Ok(config) => {
                toml::from_str::<toml::Table>(&config)
                    .with_context(|| format!("Cannot parse {}", path.display()))?;
                anyhow::ensure!(
                    !config_declares_distributed(&config)
                        && aida_core::store_locate::store_path_candidates(&config).is_empty(),
                    "Configured canonical review-story storage is unavailable at {}",
                    path.display()
                );
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err).with_context(|| format!("Cannot read {}", path.display())),
        }
    }
    let path = aida_core::resolve_requirements_path_in(
        project_root,
        None,
        None,
        std::path::Path::new(""),
    )?;
    Storage::new(path).load()
}

pub(crate) fn canonical_review_story<'a>(
    store: &'a RequirementsStore,
    forge: ReviewForge,
    number: u64,
    covered_specs: Option<&[String]>,
    preferred_spec: Option<&str>,
) -> Option<&'a aida_core::Requirement> {
    let expected: std::collections::BTreeSet<String> = covered_specs
        .unwrap_or_default()
        .iter()
        .map(|id| id.to_ascii_uppercase())
        .collect();
    let covered = |req: &aida_core::Requirement| {
        req.relationships
            .iter()
            .filter(|rel| rel.rel_type.to_string().eq_ignore_ascii_case("implements"))
            .filter_map(|rel| store.requirements.iter().find(|r| r.id == rel.target_id))
            .filter_map(|r| r.spec_id.as_deref().or(r.agreed_id.as_deref()))
            .map(|id| id.to_ascii_uppercase())
            .collect::<std::collections::BTreeSet<_>>()
    };
    let mut candidates: Vec<_> = store
        .requirements
        .iter()
        .filter(|req| {
            !is_terminal_status(&req.status) && review_title_matches(&req.title, forge, number)
        })
        .collect();
    candidates.sort_by_key(|req| {
        let coverage_rank = if covered_specs.is_some() && covered(req) != expected {
            1
        } else {
            0
        };
        let display = req.display_id();
        let preferred_rank = if preferred_spec.is_some_and(|id| id.eq_ignore_ascii_case(&display)) {
            0
        } else {
            1
        };
        (coverage_rank, preferred_rank, display.to_ascii_uppercase())
    });
    candidates.into_iter().next()
}

/// Strip ANSI SGR sequences (`ESC[...m`) so we can match output text
/// regardless of whether the child invocation colored its output. Keep it
/// minimal — we only need the SGR shape `aida add` emits.
/// trace:STORY-66 | ai:claude
pub(crate) fn strip_ansi_color(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for cc in chars.by_ref() {
                if cc.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Self-invoke `aida add` for a Story-typed review requirement and parse the
/// resulting spec id out of stdout. Returns None on any failure.
/// trace:STORY-66 | ai:claude
pub(crate) fn aida_subcmd_add_review_story(
    project_root: &std::path::Path,
    title: &str,
    description: &str,
) -> Option<String> {
    let aida = aida_exe_path();
    let out = std::process::Command::new(&aida)
        .current_dir(project_root)
        .args([
            "add",
            "--type",
            "story",
            "--status",
            "approved",
            "--priority",
            "medium",
            "--title",
            title,
            "--description",
            description,
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "{} auto-queue review story: `aida add` failed: {}",
            "Warning:".yellow().bold(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_spec_id_from_add_output(&stdout)
}

/// Parse the spec id printed by `aida add`. The git-canonical path prints
/// `Added: STORY-N - <title>` (one line); the legacy YAML/SQLite path
/// prints a standalone `ID: STORY-N` line. Accept either so this hook keeps
/// working across backends. trace:STORY-66 | ai:claude
pub(crate) fn parse_spec_id_from_add_output(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let cleaned = strip_ansi_color(line);
        let candidate = cleaned
            .strip_prefix("Added: ")
            .map(|rest| rest.split(" - ").next().unwrap_or("").trim().to_string())
            .or_else(|| {
                cleaned
                    .strip_prefix("ID: ")
                    .map(|rest| rest.trim().to_string())
            });
        if let Some(c) = candidate {
            if !c.is_empty() {
                return Some(c);
            }
        }
    }
    None
}

// ============================================================================
// TASK-96 — plan Followups extraction. When a spec reaches Done (`aida queue
// done`) or Completed (the STORY-86 auto-bump), parse the `## Followups`
// section of any matching docs/plans/ file and offer to file each bullet as
// a child TASK so out-of-scope items don't get forgotten.
// ============================================================================

// BUG-656: BUG-655's per-parent `(parent, title)` dedup was not enough. The
// auto-followups path keys child TASKs to the *completing spec*, so the same
// plan's bullets get re-filed when a DIFFERENT spec completes — either because
// the plan's header lists several specs (each "owns" the plan and re-extracts
// it) or because the same followup text was copied into a sibling plan. On
// EPIC-0428 this re-filed ~14 cross-plan duplicate TASKs (TASK-1019-1032). The
// stable signature is the PLAN PATH, not the completing spec: a plan's followups
// must be extracted exactly once across the whole store. The marker comment
// already records `extracted from <relative plan paths>`; we parse that back out
// of every spec's comments to build the already-extracted set, then skip any
// plan already in it. A re-completion, a sibling-spec completion, or a slice PR
// landing the plan twice all become no-ops at the plan granularity.

/// Best-effort `aida rel add <from> <to> --type implements` (and the
/// matching reverse `implemented-by` so `aida show <spec>` surfaces the
/// review story). Custom relation types don't have an `inverse()` mapping
/// in core, so `--bidirectional` is a no-op for them — we add both
/// directions manually instead. Logs and continues on failure.
/// trace:STORY-66 | ai:claude
pub(crate) fn aida_subcmd_rel_add_implements(project_root: &std::path::Path, from: &str, to: &str) {
    aida_subcmd_rel_add(project_root, from, to, "implements");
    aida_subcmd_rel_add(project_root, to, from, "implemented-by");
}

pub(crate) fn aida_subcmd_rel_add(
    project_root: &std::path::Path,
    from: &str,
    to: &str,
    rel_type: &str,
) {
    let aida = aida_exe_path();
    match std::process::Command::new(&aida)
        .current_dir(project_root)
        .args(["rel", "add", from, to, "--type", rel_type])
        .output_retrying_etxtbsy()
    {
        Ok(o) if o.status.success() => {}
        Ok(o) => eprintln!(
            "{} auto-queue: `rel add {} {} --type {}` failed: {}",
            "Warning:".yellow().bold(),
            from,
            to,
            rel_type,
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => eprintln!(
            "{} auto-queue: could not invoke `aida rel add`: {}",
            "Warning:".yellow().bold(),
            e
        ),
    }
}

/// `aida queue add <id> --for reviewer --no-scope --note <...>`.
///
/// The result is deliberately not best-effort: callers must not report a PR
/// as handed off until the reviewer queue durably owns it.
// trace:STORY-66 trace:BUG-1291 | ai:claude ai:codex
pub(crate) fn aida_subcmd_queue_add_for_reviewer(
    project_root: &std::path::Path,
    spec_id: &str,
    note: &str,
) -> anyhow::Result<()> {
    aida_subcmd_queue_add_for_reviewer_using(project_root, spec_id, note, &aida_exe_path())
}

pub(crate) fn aida_subcmd_queue_add_for_reviewer_using(
    project_root: &std::path::Path,
    spec_id: &str,
    note: &str,
    aida: &std::path::Path,
) -> anyhow::Result<()> {
    let out = std::process::Command::new(aida)
        .current_dir(project_root)
        .args([
            "queue",
            "add",
            spec_id,
            "--for",
            "reviewer",
            "--no-scope",
            "--note",
            note,
        ])
        // The injected `aida` may be a fixture this process just wrote, so a
        // sibling thread's forked child can still hold a writer descriptor on
        // it. Without the retry that surfaces as `could not invoke`, which is
        // the *other* error arm from the non-zero-exit one callers care about.
        // trace:BUG-1735 | ai:claude
        .output_retrying_etxtbsy();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => anyhow::bail!(
            "`aida queue add {spec_id}` failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => Err(e).context("could not invoke `aida queue add`"),
    }
}

/// What `try_auto_queue_pr_review` did, in four buckets that map to the
/// caller's display formatting (green check / cyan info / dim by-design /
/// yellow attention-needed). The fourth bucket (TASK-74) was carved out
/// of the original BUG-72 "Skipped" bucket so review sessions (whose
/// branches don't produce PRs by design) stop reading as failures, and
/// missing-tool / lookup-failed cases stop reading as routine.
/// trace:BUG-72 TASK-74 | ai:claude
pub(crate) enum AutoQueueStatus {
    /// New review story filed and queued for reviewer.
    Filed,
    /// PR found but a `Review PR-N` story already exists — idempotent re-run.
    AlreadyExists,
    /// Skipped on purpose — this session shape never produces a PR (e.g.
    /// reviewer session on a `pr-N` branch). Dimmed in output: nothing to
    /// fix, the user just needs to know the hook checked and stepped aside.
    /// trace:TASK-74 | ai:claude
    SkippedByDesign,
    /// Skipped but the user might want to do something about it: `gh`
    /// missing, `gh pr list` failed, or the queue/rel-add subprocess
    /// errored. Rendered in yellow so it doesn't blend into the by-design
    /// noise floor. trace:TASK-74 | ai:claude
    SkippedNeedsAttention,
}

/// Outcome of the end-of-session auto-queue. Always returns SOMETHING so
/// `session_end` can render a clear status line. trace:BUG-72 | ai:claude
pub(crate) struct AutoQueueOutcome {
    pub(crate) status: AutoQueueStatus,
    pub(crate) summary: String,
    /// STORY-106: PR number this outcome relates to (set on Filed and
    /// AlreadyExists). Drives the post-session "start the review"
    /// workflow hint. `None` for the skip cases.
    /// trace:STORY-106 | ai:claude
    pub(crate) pr_number: Option<u64>,
    /// TASK-267: delivered `(REQ-ID)` trailers the PR carries. Lets the
    /// post-session workflow hint name which specs `aida pull` will
    /// auto-bump Done → Completed. Populated on Filed; empty otherwise.
    /// trace:TASK-267 | ai:claude
    pub(crate) covered_specs: Vec<String>,
    /// Canonical story selected or filed for this change request.
    /// trace:BUG-1817 | ai:codex
    pub(crate) review_spec: Option<String>,
}

impl AutoQueueOutcome {
    pub(crate) fn filed(s: impl Into<String>) -> Self {
        Self {
            status: AutoQueueStatus::Filed,
            summary: s.into(),
            pr_number: None,
            covered_specs: Vec::new(),
            review_spec: None,
        }
    }
    pub(crate) fn already_exists(s: impl Into<String>) -> Self {
        Self {
            status: AutoQueueStatus::AlreadyExists,
            summary: s.into(),
            pr_number: None,
            covered_specs: Vec::new(),
            review_spec: None,
        }
    }
    /// STORY-106: attach the PR number that this outcome refers to. Used
    /// by `session_end` to emit a concrete "start the review" hint.
    pub(crate) fn with_pr(mut self, pr_number: u64) -> Self {
        self.pr_number = Some(pr_number);
        self
    }
    /// TASK-267: attach the delivered spec IDs the PR covers, so the
    /// session-end hint names them in the `aida pull` auto-bump line.
    /// trace:TASK-267 | ai:claude
    pub(crate) fn with_specs(mut self, specs: Vec<String>) -> Self {
        self.covered_specs = specs;
        self
    }
    pub(crate) fn with_review_spec(mut self, spec: impl Into<String>) -> Self {
        self.review_spec = Some(spec.into());
        self
    }
    /// Skip the user shouldn't care about (review session, no PR-producing
    /// branch). Rendered dim. trace:TASK-74 | ai:claude
    pub(crate) fn skipped_by_design(s: impl Into<String>) -> Self {
        Self {
            status: AutoQueueStatus::SkippedByDesign,
            summary: s.into(),
            pr_number: None,
            covered_specs: Vec::new(),
            review_spec: None,
        }
    }
    /// Skip the user might want to act on (missing tool, gh failed, queue
    /// subprocess error). Rendered yellow. trace:TASK-74 | ai:claude
    pub(crate) fn skipped_needs_attention(s: impl Into<String>) -> Self {
        Self {
            status: AutoQueueStatus::SkippedNeedsAttention,
            summary: s.into(),
            pr_number: None,
            covered_specs: Vec::new(),
            review_spec: None,
        }
    }
}

/// Heuristic: does this branch shape correspond to a session that, by
/// design, never produces a PR? Today: `pr-N`, `mr-N`, `github-N`,
/// `gitlab-N` — the local branch names `aida session start --owns PR-N`
/// creates for review sessions. Case-insensitive. trace:TASK-74 | ai:claude
pub(crate) fn is_review_session_branch(branch: &str) -> bool {
    let lower = branch.to_ascii_lowercase();
    let prefixes = ["pr-", "mr-", "github-", "gitlab-"];
    prefixes.iter().any(|p| {
        lower
            .strip_prefix(p)
            .map(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false)
    })
}

/// Which pre-flight condition, if any, says "do NOT file a review story".
///
/// The auto-file used to run unconditionally, so a repo with no `origin`
/// remote produced a `Review PR-0: ` story with an empty subject covering
/// no specs — on EVERY `aida session end`. Three conditions gate it now,
/// each of them meaning "there is no reviewable change here":
///
/// - no `origin` remote — a local-only project has nothing to open a PR
///   against (the same `has_remote(.., "origin")` condition `aida db sync
///   --pull` uses to skip its pull),
/// - the resolved change id is `0` — the pure-git provider's synthetic
///   "the branch IS the change" ref, i.e. the forge has no PR concept,
/// - the story would cover no specs — no `(REQ-ID)` trailers anywhere in
///   the PR's commit range, so the reviewer story carries no linkage.
///
/// Pure so the policy is unit-testable without a git repo or a forge.
/// `pr_number` / `covered_specs` are `None` at the call sites that run
/// before those facts are known.
// trace:BUG-776 | ai:claude
/// BUG-1223: map the detected forge onto the review-side forge enum. GitLab
/// keeps its own arm; GitHub and pure-git both take the GitHub arm (pure-git
/// has no PR concept and is filtered out earlier by the synthetic change id).
/// Pure so the mapping is unit-testable.
// trace:BUG-1223 | ai:claude
pub(crate) fn review_forge_for_kind(kind: crate::forge::ForgeKind) -> ReviewForge {
    match kind {
        crate::forge::ForgeKind::GitLab => ReviewForge::GitLab,
        _ => ReviewForge::GitHub,
    }
}

pub(crate) fn auto_queue_skip_reason(
    has_origin_remote: bool,
    pr_number: Option<u64>,
    covered_specs: Option<usize>,
) -> Option<String> {
    if !has_origin_remote {
        return Some(
            "auto-queue: no `origin` remote (local-only project) — no PR can exist, so no reviewer story was filed"
                .to_string(),
        );
    }
    if pr_number == Some(0) {
        return Some(
            "auto-queue: this forge has no pull-request concept (synthetic change id 0) — no reviewer story was filed"
                .to_string(),
        );
    }
    if let (Some(n), Some(0)) = (pr_number, covered_specs) {
        return Some(format!(
            "auto-queue: PR #{n} carries no `(REQ-ID)` trailers in its commit range — no reviewer story was filed (add a trailer, then re-run `aida pr auto-queue-review`)"
        ));
    }
    None
}

#[cfg(test)]
#[path = "tests/bug776_auto_queue_gate_tests.rs"]
mod bug776_auto_queue_gate_tests;

/// Where the auto-queue was triggered from. Used purely for the
/// description blurb on the filed review story (so a reviewer reading
/// it later can tell "the /aida-pr skill filed me at PR-create time"
/// from "session end picked me up as a backup"). Doesn't change any
/// other behavior. trace:STORY-90 | ai:claude
#[derive(Debug, Clone, Copy)]
pub(crate) enum AutoQueueOrigin {
    /// Fired from `aida pr auto-queue-review`, normally by /aida-pr right
    /// after `gh pr create`.
    PrSkill,
    /// Fired from `aida session end` as the idempotent backup trigger.
    SessionEnd,
}

impl AutoQueueOrigin {
    pub(crate) fn human_origin(self) -> &'static str {
        match self {
            AutoQueueOrigin::PrSkill => "`aida pr auto-queue-review` (typically from /aida-pr)",
            AutoQueueOrigin::SessionEnd => "`aida session end` (backup trigger)",
        }
    }
}

/// Auto-detect-and-queue. Triggered primarily from /aida-pr right after
/// `gh pr create` returns, and as an idempotent backup from `aida session
/// end` so a forgotten `gh pr create` (or a manual one outside the skill)
/// doesn't leave the reviewer unaware. Returns an outcome describing what
/// happened — see `AutoQueueStatus` for the categories.
/// trace:STORY-66 STORY-90 BUG-72 | ai:claude
/// Pick a working directory the end-of-session auto-queue can safely
/// shell out from. It runs `gh` / `git` / `aida` subprocesses, every one
/// of which fails with a misleading ENOENT (blamed on the binary, not the
/// cwd) if its working directory was removed. `aida session end` removes
/// the session worktree, so when that worktree was the invocation dir we
/// fall back to the lease's recorded parent project root — the main
/// worktree, which is never removed. Returns the first candidate that is
/// still a real directory, or `None` when both are gone.
/// trace:BUG-107 | ai:claude
pub(crate) fn auto_queue_working_dir(
    parent_project_root: Option<&std::path::Path>,
    invocation_root: &std::path::Path,
) -> Option<std::path::PathBuf> {
    [parent_project_root, Some(invocation_root)]
        .into_iter()
        .flatten()
        .find(|p| p.is_dir())
        .map(|p| p.to_path_buf())
}

// BUG-1944: callers use `live_work::try_auto_queue_pr_review`, which wraps
// this and moves the PR's specs to In Progress. trace:BUG-1944 | ai:claude
pub(crate) fn try_auto_queue_pr_review_unbumped(
    project_root: &std::path::Path,
    branch: &str,
    session_id: &str,
    origin: AutoQueueOrigin,
) -> AutoQueueOutcome {
    // TASK-74: short-circuit when the branch shape says "this session
    // never produces a PR" (reviewer session on `pr-N` / `mr-N` etc.).
    // Catches the case before we even call gh, so a reviewer with `gh`
    // uninstalled doesn't get a misleading "gh not found" warning when
    // the right answer is "we wouldn't have filed anyway".
    if is_review_session_branch(branch) {
        return AutoQueueOutcome::skipped_by_design(format!(
            "auto-queue: reviewer session on `{}` — no PR to file (skip by design)",
            branch
        ));
    }

    // BUG-776: a repo with no `origin` remote cannot have a PR, but the
    // pure-git forge provider still answers `change_for_branch` with a
    // synthetic id-0 ChangeRef — which used to mint a `Review PR-0: ` story
    // covering no specs on every single `aida session end`. Gate the whole
    // path on the same remote check `aida db sync --pull` uses before it
    // even talks to the forge. trace:BUG-776 | ai:claude
    if let Some(reason) = auto_queue_skip_reason(
        aida_core::git_ops::has_remote(project_root, "origin"),
        None,
        None,
    ) {
        return AutoQueueOutcome::skipped_by_design(reason);
    }

    // trace:BUG-1807 | ai:codex
    let kind = crate::forge::resolve_forge_kind(project_root);
    let noun = kind.change_noun();
    let cli = kind.cli_name();

    // STORY-516: forge-routed. Reconstruct OpenPrInfo from the ChangeRef so the
    // downstream pr.number/url/title uses stay unchanged. trace:STORY-516 | ai:claude
    let pr = match change_lookup_for_branch(project_root, branch) {
        crate::forge::ChangeLookup::Found(c) => OpenPrInfo {
            number: c.id,
            title: c.title.unwrap_or_default(),
            url: c.url,
            head_branch: (!c.branch.is_empty()).then_some(c.branch),
        },
        crate::forge::ChangeLookup::NoChange => {
            return AutoQueueOutcome::skipped_by_design(format!(
                "auto-queue: no open {noun} for branch `{}` — reviewer queue not filed",
                branch
            ));
        }
        crate::forge::ChangeLookup::CliMissing => {
            return AutoQueueOutcome::skipped_needs_attention(format!(
                "auto-queue: `{cli}` CLI not on PATH — would have queued reviewer story for branch `{}`. Install {cli} to enable.",
                branch
            ));
        }
        crate::forge::ChangeLookup::CliFailed(reason) => {
            return AutoQueueOutcome::skipped_needs_attention(format!(
                "auto-queue: `{cli}` lookup failed for branch `{}` ({}) — no reviewer story filed",
                branch, reason
            ));
        }
        crate::forge::ChangeLookup::Unreachable(reason) => {
            return AutoQueueOutcome::skipped_needs_attention(format!(
                "auto-queue: {noun} API unreachable for branch `{}` ({}) — no reviewer story filed (transient; retry once the API is reachable)",
                branch, reason
            ));
        }
    };
    // BUG-776: a remote that is not a PR-speaking forge (bare git server,
    // GitLab-over-plain-git) resolves to the synthetic id-0 ChangeRef. There
    // is no PR #0 to review — skip before the base/head probe so we don't
    // also emit its "couldn't resolve PR base/head" warning.
    // trace:BUG-776 | ai:claude
    if let Some(reason) = auto_queue_skip_reason(true, Some(pr.number), None) {
        return AutoQueueOutcome::skipped_by_design(reason);
    }
    // Pull the commit-range spec ids using the existing helpers from STORY-67.
    // BUG-85: split into "delivered" (subject `(REQ-ID)` parens) and
    // "referenced" (body content / trace comments). Only delivered IDs count
    // toward "covers N specs" or get an `implements` relation — referenced
    // IDs are informational (spot-check for regressions).
    // BUG-1223: this used to hard-code `ReviewForge::GitHub`, so on a GitLab
    // project the base/head lookup ran `gh pr view` against the wrong forge,
    // the fallback range covered nothing, and a trailered MR was reported as
    // "carries no `(REQ-ID)` trailers" — no `Review PR-N` story, so the
    // drain's reviewer phase had nothing to pick up. Resolve the forge the
    // same way the review-prompt path does (config `[forge]`, else origin).
    // trace:BUG-1223 trace:TASK-1254 | ai:claude
    let review_forge = review_forge_for_kind(kind);
    let (base, head) = pr_base_head(project_root, review_forge, pr.number)
        .unwrap_or_else(|_| ("main".to_string(), branch.to_string()));
    let messages = git_log_messages(project_root, &base, &head).unwrap_or_default();
    let mut spec_ids: Vec<String> = Vec::new();
    let mut referenced_ids: Vec<String> = Vec::new();
    for msg in &messages {
        for id in extract_spec_ids_from_commit(msg) {
            if !spec_ids.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
                spec_ids.push(id);
            }
        }
        for id in extract_referenced_spec_ids_from_commit(msg) {
            if !referenced_ids.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
                referenced_ids.push(id);
            }
        }
    }
    // A referenced ID delivered by ANOTHER commit in the same range is
    // still delivered overall — drop it from references to keep the lists
    // disjoint across the range. trace:BUG-85 | ai:claude
    referenced_ids.retain(|r| !spec_ids.iter().any(|d| d.eq_ignore_ascii_case(r)));

    // BUG-776: a story that covers no specs is unlinked noise in the reviewer
    // queue — the reviewer has nothing to check acceptance against, and the
    // row has to be dequeued and rejected by hand. Skip it and say why, so the
    // fix ("add a `(REQ-ID)` trailer") is obvious. trace:BUG-776 | ai:claude
    if let Some(reason) = auto_queue_skip_reason(true, Some(pr.number), Some(spec_ids.len())) {
        return AutoQueueOutcome::skipped_by_design(reason);
    }

    // Read canonical objects for both distributed and legacy stores. A failed
    // lookup cannot establish absence: do not file a duplicate on read failure.
    // trace:BUG-1807 | ai:codex
    let store = match load_review_story_inventory(project_root) {
        Ok(store) => store,
        Err(err) => {
            return AutoQueueOutcome::skipped_needs_attention(format!(
                "auto-queue: cannot read review stories for {noun}-{} — reviewer handoff not confirmed: {err:#}",
                pr.number
            ));
        }
    };
    {
        let persisted =
            drain_state::DrainState::read(project_root).and_then(|state| state.review_spec);
        if let Some(existing) = canonical_review_story(
            &store,
            review_forge,
            pr.number,
            Some(&spec_ids),
            persisted.as_deref(),
        ) {
            let existing_id = existing.display_id();
            let note = format!(
                "auto-queue retry reused canonical review story; covers {} spec{}",
                spec_ids.len(),
                if spec_ids.len() == 1 { "" } else { "s" }
            );
            if let Err(err) = aida_subcmd_queue_add_for_reviewer(project_root, &existing_id, &note)
            {
                return AutoQueueOutcome::skipped_needs_attention(format!(
                        "auto-queue: canonical review story {existing_id} exists for {} but reviewer queue insertion failed: {err}",
                        format_review_label(review_forge, pr.number)
                    ))
                    .with_pr(pr.number)
                    .with_specs(spec_ids)
                    .with_review_spec(existing_id);
            }
            return AutoQueueOutcome::already_exists(format!(
                "{} #{} reuses canonical review story {}",
                format_review_label(review_forge, pr.number)
                    .split_once('-')
                    .map(|(prefix, _)| prefix)
                    .unwrap_or("PR"),
                pr.number,
                existing_id
            ))
            .with_pr(pr.number)
            .with_specs(spec_ids)
            .with_review_spec(existing_id);
        }
    }

    let session_short: &str = &session_id[..session_id.len().min(8)];
    let mut desc = String::new();
    desc.push_str(&format!(
        "Auto-filed by {} (session `{}`) for branch `{}`.\n\n",
        origin.human_origin(),
        session_short,
        branch
    ));
    desc.push_str(&format!("- {noun}: <{}>\n", pr.url));
    desc.push_str(&format!("- Branch: `{}` → `{}`\n\n", head, base));
    if spec_ids.is_empty() {
        // BUG-776 gates this branch off — a zero-coverage story is no longer
        // filed at all. Kept as a defensive fallback so relaxing the gate
        // can't ship a story with an empty body. trace:BUG-776 | ai:claude
        desc.push_str(
            "No `(REQ-ID)` trailers were found in the PR's commit range — review against the PR title/body and link specs after the fact via `aida rel add <this> <spec> --type implements`.\n\n",
        );
    } else {
        desc.push_str("## Covers\n\n");
        for id in &spec_ids {
            desc.push_str(&format!("- {}\n", id));
        }
        desc.push('\n');
    }
    if !referenced_ids.is_empty() {
        desc.push_str("## References (not delivered)\n\n");
        desc.push_str(
            "Spec IDs mentioned in commit bodies or trace comments — touched but not delivered by this PR. Spot-check for regressions.\n\n",
        );
        for id in &referenced_ids {
            desc.push_str(&format!("- {}\n", id));
        }
        desc.push('\n');
    }
    desc.push_str("## Acceptance\n\n");
    desc.push_str(&format!(
        "- Generate a structured review prompt with `aida review prompt --pr {}` and verify each spec's acceptance criteria.\n",
        pr.number
    ));
    desc.push_str("- Approve and merge, or request changes by spec id.\n");
    desc.push_str("- Mark this story `completed` once the PR is merged.\n");

    // BUG-1609: the story title carries the forge-correct label ("Review
    // MR-N" for GitLab), not a hardcoded "Review PR-N". Downstream,
    // `parse_review_scope` reads this prefix back into `plan.review_target`'s
    // `ReviewForge` — a GitHub-shaped title on a GitLab MR silently made the
    // whole reviewer-preflight chain (stale-base check, intermediate-only
    // check, `pr_base_head`) treat the MR as a GitHub PR and shell out to
    // `gh`, which then fabricated a GitHub-shaped `pr-N` fallback head for a
    // branch `gh` had never heard of. trace:BUG-1609 | ai:claude
    let title = format!(
        "Review {}: {}",
        format_review_label(review_forge, pr.number),
        pr.title
    );
    let new_id = match aida_subcmd_add_review_story(project_root, &title, &desc) {
        Some(id) => id,
        None => {
            return AutoQueueOutcome::skipped_needs_attention(format!(
                "auto-queue: `aida add` failed for {noun}-{} (see warning above)",
                pr.number
            ));
        }
    };

    for id in &spec_ids {
        aida_subcmd_rel_add_implements(project_root, &new_id, id);
    }

    let origin_short = match origin {
        AutoQueueOrigin::PrSkill => "aida pr",
        AutoQueueOrigin::SessionEnd => "session end",
    };
    let note = format!(
        "auto-queued by `{}` ({}); covers {} spec{}",
        origin_short,
        session_short,
        spec_ids.len(),
        if spec_ids.len() == 1 { "" } else { "s" }
    );
    if let Err(err) = aida_subcmd_queue_add_for_reviewer(project_root, &new_id, &note) {
        return AutoQueueOutcome::skipped_needs_attention(format!(
            "auto-queue: filed {new_id} for {noun}-{} but reviewer queue insertion failed: {err}; the unqueued story remains a durable retry signal",
            pr.number
        ))
        .with_pr(pr.number)
        .with_specs(spec_ids)
        .with_review_spec(new_id);
    }

    let covers = if spec_ids.is_empty() {
        "no specs".to_string()
    } else {
        spec_ids.join(", ")
    };
    AutoQueueOutcome::filed(format!(
        "filed {} (covers {}) → reviewer queue ({noun}-{})",
        new_id, covers, pr.number
    ))
    .with_pr(pr.number)
    .with_specs(spec_ids)
    .with_review_spec(new_id)
}

// trace:BUG-1807 | ai:codex
pub(crate) fn require_review_handoff(
    outcome: &AutoQueueOutcome,
) -> Result<(), auto_complete::PhaseFailure> {
    match outcome.status {
        AutoQueueStatus::Filed | AutoQueueStatus::AlreadyExists => Ok(()),
        _ => Err(auto_complete::PhaseFailure::new(format!(
            "reviewer handoff not confirmed: {}; retry `aida pr auto-queue-review`",
            outcome.summary
        ))),
    }
}

/// Print an `AutoQueueOutcome` in the convention shared by `aida session
/// end` and `aida pr auto-queue-review` (filed = green check, already-exists
/// = cyan info, by-design skip = dim, needs-attention = yellow warning).
/// trace:STORY-90 BUG-72 | ai:claude
pub(crate) fn render_auto_queue_outcome(outcome: &AutoQueueOutcome) {
    match outcome.status {
        AutoQueueStatus::Filed => println!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            outcome.summary
        ),
        AutoQueueStatus::AlreadyExists => println!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            outcome.summary
        ),
        AutoQueueStatus::SkippedByDesign => {
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::Info).dimmed(),
                outcome.summary.dimmed()
            )
        }
        AutoQueueStatus::SkippedNeedsAttention => {
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                outcome.summary.yellow()
            )
        }
    }
}

/// `git -C <root> branch --show-current` — used by `aida pr` when the
/// caller doesn't pass --branch. Returns trimmed branch name on success;
/// empty string when detached / not a repo. trace:STORY-90 | ai:claude
pub(crate) fn current_git_branch(project_root: &std::path::Path) -> Result<String> {
    let out = std::process::Command::new("git")
        .current_dir(project_root)
        .args(["branch", "--show-current"])
        .output()
        .context("could not invoke `git branch --show-current` — is git installed?")?;
    if !out.status.success() {
        anyhow::bail!(
            "`git branch --show-current` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// BUG-677: `LeaseState` + `classify_lease_state` moved to `aida_core::liveness`
/// (re-exported at the crate root, see the top-of-file `pub(crate) use`). The
/// pure enum + classifier is now shared with `aida-tui`; only the CLI glyph
/// rendering stays here, as an extension trait (glyph() reaches into the CLI
/// glyph palette, which aida-core has no business knowing). `label()` moved with
/// the enum (pure strings).
// trace:BUG-677 | ai:claude
pub(crate) trait LeaseStateGlyph {
    fn glyph(self) -> &'static str;
}

impl LeaseStateGlyph for LeaseState {
    fn glyph(self) -> &'static str {
        match self {
            LeaseState::Live => "●",
            LeaseState::Dormant => crate::glyph(crate::glyphs::Glyph::InFlight),
            LeaseState::Stale => crate::glyph(crate::glyphs::Glyph::Warning),
        }
    }
}

/// TASK-56: find the Claude Code session id matching a worktree path,
/// derived from the newest `*.jsonl` file at
/// `~/.claude/projects/<encoded-cwd>/`. Returns None when the project
/// dir doesn't exist or has no jsonl files. trace:TASK-56 | ai:claude
pub(crate) fn cc_session_id_for_worktree(worktree: &std::path::Path) -> Option<String> {
    let dir = session::claude_project_dir(worktree).ok()?;
    if !dir.exists() {
        return None;
    }
    let mut newest: Option<(std::time::SystemTime, String)> = None;
    for entry in std::fs::read_dir(&dir).ok()? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if stem.is_empty() {
            continue;
        }
        match &newest {
            Some((best_mtime, _)) if mtime <= *best_mtime => {}
            _ => newest = Some((mtime, stem)),
        }
    }
    newest.map(|(_, s)| s)
}

/// STORY-637: render the cross-clone lease section — claims on the shared
/// `aida-store` registry held by OTHER clones (a different clone path than
/// ours). Returns the number of foreign claims shown. Distinct from the
/// local-lease table above it. trace:STORY-637 | ai:claude
///
/// BUG-1764: stale claims are now EXCLUDED by default. This section applied no
/// expiry predicate of any kind — its only filter was `clone_path != ours`, and
/// while it computed `age` from `heartbeat_at` for the display column it never
/// used it as a predicate, so a foreign claim rendered as active forever once
/// written. `show_all` (`--all`) brings them back with the staleness reason,
/// mirroring the contract the LOCAL lease table above already has for `--all`.
// trace:BUG-1764 | ai:claude
pub(crate) fn print_cross_clone_leases(
    project_root: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
    show_all: bool,
) -> usize {
    let store_root = project_root.join(".aida-store");
    // Two clones sharing one store inherit the same node id, so discriminate
    // "ours" vs "foreign" by the canonical clone path (the project root).
    let our_clone = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .display()
        .to_string();
    let foreign: Vec<coordination::Claim> = coordination::list_claims(&store_root)
        .into_iter()
        .filter(|c| c.clone_path.is_empty() || c.clone_path != our_clone)
        .collect();
    // BUG-1764: split before rendering. `partition_claims` is keyed on `host`,
    // not `clone_path`, so a foreign-CLONE claim on THIS host is evaluated
    // against the local process table rather than excused as unreachable.
    let (live, stale) = coordination::partition_claims(foreign, now, &coordination::hostname());
    let shown: Vec<(&coordination::Claim, Option<&str>)> = if show_all {
        live.iter()
            .map(|c| (c, None))
            .chain(stale.iter().map(|(c, r)| (c, Some(r.as_str()))))
            .collect()
    } else {
        live.iter().map(|c| (c, None)).collect()
    };
    if shown.is_empty() {
        // Nothing live, but say so rather than leaving the operator to wonder
        // why a lease they can see on disk never appears.
        if !stale.is_empty() {
            println!();
            println!(
                "{} {} stale cross-clone lease{} hidden · {} to list, {} to release",
                "·".dimmed(),
                stale.len(),
                if stale.len() == 1 { "" } else { "s" },
                "aida session leases --all".cyan(),
                "aida session leases --prune-stale".cyan()
            );
        }
        return 0;
    }
    println!();
    println!("{}", "Cross-clone leases (other clones)".bold());
    println!();
    if show_all {
        println!(
            "{:<20} {:<10} {:<14} {:<14} {:<10} state",
            "scope", "host", "node", "agent", "age"
        );
    } else {
        println!(
            "{:<20} {:<10} {:<14} {:<14} age",
            "scope", "host", "node", "agent"
        );
    }
    println!("{}", "─".repeat(if show_all { 86 } else { 72 }));
    for (c, stale_reason) in &shown {
        let age = chrono::DateTime::parse_from_rfc3339(&c.heartbeat_at)
            .ok()
            .map(|t| {
                let secs = now
                    .signed_duration_since(t.with_timezone(&chrono::Utc))
                    .num_seconds()
                    .max(0);
                format!("{secs}s")
            })
            .unwrap_or_else(|| "?".to_string());
        if show_all {
            let state = match stale_reason {
                Some(reason) => format!("{} {}", "⚠ stale".yellow(), reason.dimmed()),
                None => format!("{}", "● live".green()),
            };
            println!(
                "{:<20} {:<10} {:<14} {:<14} {:<10} {}",
                truncate(&c.scope, 20),
                truncate(&c.host, 10),
                truncate(&c.node_id, 14),
                truncate(if c.agent.is_empty() { "-" } else { &c.agent }, 14),
                age,
                state
            );
        } else {
            println!(
                "{:<20} {:<10} {:<14} {:<14} {}",
                truncate(&c.scope, 20),
                truncate(&c.host, 10),
                truncate(&c.node_id, 14),
                truncate(if c.agent.is_empty() { "-" } else { &c.agent }, 14),
                age
            );
        }
        println!("{}{}", " ".repeat(2), c.clone_path.dimmed());
    }
    if !show_all && !stale.is_empty() {
        println!(
            "{} {} stale cross-clone lease{} hidden · {} to list, {} to release",
            "·".dimmed(),
            stale.len(),
            if stale.len() == 1 { "" } else { "s" },
            "aida session leases --all".cyan(),
            "aida session leases --prune-stale".cyan()
        );
    }
    shown.len()
}

/// TASK-345: build the `drain` cross-reference for a lease whose scope is the
/// active drain's current member, or `None` when this lease is not the drain's
/// active member (or no drain is live). Lets `aida session leases --json`
/// correlate a lease to the orchestrator drain/run driving it — `run_uuid`
/// (the per-spec orchestration id), the current phase, the drain mode/batch,
/// the launching command, the orchestrator pid, and its start time.
// trace:TASK-345 | ai:claude
pub(crate) fn lease_drain_xref(
    scope: &str,
    drain: Option<&drain_state::DrainState>,
) -> Option<serde_json::Value> {
    let d = drain?;
    // Only the drain's *current* member is an orchestrator phase child.
    if d.current.as_deref() != Some(scope) {
        return None;
    }
    Some(serde_json::json!({
        "run_uuid": (!d.run_uuid.is_empty()).then(|| d.run_uuid.clone()),
        "member": d.current.clone(),
        "phase": d.current_phase.clone(),
        "mode": d.mode.clone(),
        "batch": d.batch.clone(),
        "command": d.command.clone(),
        "orchestrator_pid": d.orchestrator_pid,
        "started_at": d.started_at.clone(),
    }))
}

/// TASK-345: assemble the `aida session leases --json` rows. Pure over the
/// classified leases + optional live drain snapshot so the JSON shape is
/// unit-testable. Each row mirrors the human columns (id/scope/branch/role/
/// worktree/state/pid) and adds the `drain` cross-reference (null for leases
/// the orchestrator isn't currently driving).
// trace:TASK-345 | ai:claude
pub(crate) fn leases_json_rows(
    shown: &[(SessionLease, LeaseState, Option<u32>)],
    drain: Option<&drain_state::DrainState>,
) -> Vec<serde_json::Value> {
    shown
        .iter()
        .map(|(l, state, pid)| {
            serde_json::json!({
                "id": l.id,
                "scope": l.scope,
                "branch": l.branch,
                "role": l.role,
                "worktree": l.worktree_path.display().to_string(),
                "state": state.label(),
                "pid": pid,
                "drain": lease_drain_xref(&l.scope, drain),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/task345_leases_json_tests.rs"]
mod task345_leases_json_tests;

#[cfg(test)]
#[path = "tests/bug_1772_harness_worktree_lease_tests.rs"]
mod bug_1772_harness_worktree_lease_tests;

/// BUG-1764 clause 4: `aida session leases --prune-stale` — release every
/// CROSS-CLONE claim the staleness predicate reports dead.
///
/// Before this there was no supported way to clear one. `aida session end
/// --spec BUG-1689` answers `No lease found for spec ...` because a foreign
/// claim has no local lease to resolve; `aida session reap` requires spec
/// Done/Completed + branch merged + process exited; and
/// `coordination::release_claim` refuses a claim whose `clone_path` is not ours
/// by design. The only remedy was deleting
/// `.aida-store/coordination/leases/*.toml` by hand.
///
/// Opt-in and never automatic: releasing another clone's claim is a cross-clone
/// mutation. It can only ever touch a claim the SHARED predicate reports stale,
/// so there is no `--force` to add.
// trace:BUG-1764 | ai:claude
pub(crate) fn session_leases_prune_stale(yes: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let store_root = project_root.join(".aida-store");
    let our_clone = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .display()
        .to_string();
    let foreign: Vec<coordination::Claim> = coordination::list_claims(&store_root)
        .into_iter()
        .filter(|c| c.clone_path.is_empty() || c.clone_path != our_clone)
        .collect();
    let (_live, stale) =
        coordination::partition_claims(foreign, chrono::Utc::now(), &coordination::hostname());

    // BUG-1772 AC2: `aida session leases --prune-stale` also learns to reap
    // immortal pid-less harness-worktree leases.
    // trace:BUG-1772 | ai:antigravity
    let local_leases = list_leases(&project_root);
    let mut stale_local_harness_leases = vec![];
    let harness_threshold = chrono::Utc::now() - chrono::Duration::hours(24);
    for lease in local_leases {
        if lease.scope == worktree_lease::HARNESS_WORKTREE_SCOPE
            && lease.active_pid.is_none()
            && lease.creator_pid.is_none()
            && lease.started_at < harness_threshold
        {
            stale_local_harness_leases.push(lease);
        }
    }

    if stale.is_empty() && stale_local_harness_leases.is_empty() {
        println!("No stale leases to release.");
        return Ok(());
    }
    if !stale.is_empty() {
        println!(
            "{} stale cross-clone lease{} to release:",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" }
        );
        for (c, reason) in &stale {
            println!(
                "  {} {} {}",
                truncate(&c.scope, 20).yellow(),
                format!("({})", c.host).dimmed(),
                reason.dimmed()
            );
        }
    }
    if !stale_local_harness_leases.is_empty() {
        println!(
            "{} stale pid-less local harness-worktree lease{} to release:",
            stale_local_harness_leases.len(),
            if stale_local_harness_leases.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        for l in &stale_local_harness_leases {
            println!("  {} {}", l.id.yellow(), "age > 24h".dimmed());
        }
    }

    // Same posture as `aida session end`: a non-terminal stdin must not be
    // able to release a peer's claims without saying so explicitly.
    if !yes {
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            anyhow::bail!(
                "refusing to prune non-interactively without --yes (releasing another clone's \
                 claim is a cross-clone mutation)"
            );
        }
        let total = stale.len() + stale_local_harness_leases.len();
        if !confirm(&format!("Release {} stale lease(s)? [y/N] ", total)) {
            println!("Aborted.");
            return Ok(());
        }
    }
    let mut released = 0usize;
    for (c, reason) in &stale {
        match coordination::release_stale_foreign_claim(
            &store_root,
            &c.scope,
            &c.clone_path,
            &c.heartbeat_at,
        ) {
            Ok(true) => {
                released += 1;
                println!(
                    "  {} released cross-clone {} — {}",
                    "✓".green(),
                    c.scope,
                    reason
                );
            }
            // Refreshed, reclaimed, or already gone between the listing and the
            // delete — the verdict no longer describes the record on disk.
            Ok(false) => println!(
                "  {} skipped cross-clone {} — changed on the store since it was listed",
                "·".dimmed(),
                c.scope
            ),
            Err(e) => eprintln!(
                "  {} could not release cross-clone {}: {e}",
                "Warning:".yellow().bold(),
                c.scope
            ),
        }
    }
    let mut local_released = 0usize;
    for l in &stale_local_harness_leases {
        let p = lease_path(&project_root, &l.id);
        if p.exists() {
            if let Err(e) = std::fs::remove_file(&p) {
                eprintln!(
                    "  {} could not remove {}: {e}",
                    "Warning:".yellow().bold(),
                    l.id
                );
            } else {
                local_released += 1;
                println!("  {} released local harness lease {}", "✓".green(), l.id);
            }
        }
    }

    println!();
    println!(
        "Released {} of {} stale cross-clone lease claim(s), and {} of {} stale local harness lease(s).",
        released,
        stale.len(),
        local_released,
        stale_local_harness_leases.len()
    );
    Ok(())
}
pub(crate) fn session_leases(verbose: bool, all: bool, json: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let leases = list_leases(&project_root);
    if leases.is_empty() {
        // TASK-345: JSON consumers get an empty array, not the human hints.
        // trace:TASK-345 | ai:claude
        if json {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!([]))?);
            return Ok(());
        }
        // STORY-637: even with no LOCAL leases, a peer clone may hold a
        // cross-clone lease — surface it before the "no sessions" hint so a
        // refused `session start` has a place to point.
        let foreign = print_cross_clone_leases(&project_root, chrono::Utc::now(), all);
        if foreign == 0 {
            println!("(no active sessions)");
        }
        println!();
        println!(
            "Start one with: {} {}",
            "aida session start --owns".cyan(),
            "<scope>".dimmed()
        );
        // BUG-98: explicit pointer at the historical-conversations view so
        // users who landed here looking for `list` know where it lives.
        eprintln!();
        eprintln!(
            "{}",
            "(for the historical list of Claude Code conversations in this project, run `aida session conversations`)"
                .dimmed()
        );
        return Ok(());
    }

    // TASK-55: probe live claudes once + classify every lease so we
    // can render the state column AND apply the hide-stale filter
    // off the same data. trace:TASK-55 | ai:claude
    let now = chrono::Utc::now();
    let live = process_probe::probe_live_claude_sessions();
    let classified: Vec<(SessionLease, LeaseState, Option<u32>)> = leases
        .iter()
        .map(|l| {
            let live_in_worktree = live.iter().find(|s| {
                !s.stale_cwd && (s.cwd == l.worktree_path || s.cwd.starts_with(&l.worktree_path))
            });
            // BUG-511: review-verb leases classify by creator PID, not worktree.
            let state = lease_state_for(l, &live, now);
            let pid = l
                .active_pid
                .filter(|p| process_probe::pid_is_alive(*p))
                .or_else(|| live_in_worktree.map(|s| s.pid))
                .or(if l.review_verb || l.claim_verb {
                    l.creator_pid
                } else {
                    None
                });
            (l.clone(), state, pid)
        })
        .collect();

    let (shown, hidden_stale): (Vec<_>, Vec<_>) = if all {
        (classified, Vec::new())
    } else {
        classified
            .into_iter()
            .partition(|(_, state, _)| !matches!(state, LeaseState::Stale))
    };

    // STORY-301: cross-reference the active `--auto-complete` drain. A lease
    // whose scope is the drain's current member is an orchestrator phase
    // child — annotate it so `aida session leases` shows which session the
    // orchestrator is driving, and at which phase. trace:STORY-301 | ai:claude
    // TASK-345: retain the full live DrainState so the `--json` path can emit
    // the drain cross-reference (run uuid / mode / orchestrator pid) per lease,
    // not just the phase string the human view needs. trace:TASK-345 | ai:claude
    let drain_active: Option<drain_state::DrainState> = match find_main_worktree_root()
        .map(|r| drain_state::probe(&r))
        .unwrap_or(drain_state::DrainStatus::None)
    {
        drain_state::DrainStatus::Active(s) => Some(s),
        _ => None,
    };
    let drain_current: Option<(String, Option<String>)> = drain_active
        .as_ref()
        .and_then(|s| s.current.clone().map(|c| (c, s.current_phase.clone())));

    // TASK-345: machine-readable lease list. Each row carries a `drain`
    // cross-reference so a consumer can correlate a lease to the orchestrator
    // drain/run driving it. Emitted before any human rendering.
    // trace:TASK-345 | ai:claude
    if json {
        let rows = leases_json_rows(&shown, drain_active.as_ref());
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!(rows))?
        );
        return Ok(());
    }

    println!("{}", "Active session leases".bold());
    println!();
    // STORY-65: role column shows the persona inherited at session start.
    // TASK-55: adds a `state` column when --all is set. TASK-56: adds
    // `pid` + `cc-session` columns when --verbose is set.
    // trace:STORY-65, TASK-55, TASK-56 | ai:claude
    let mut header = format!(
        "{:<14} {:<20} {:<18} {:<14}",
        "id", "scope", "branch", "role"
    );
    if all {
        header.push_str(&format!(" {:<10}", "state"));
    }
    if verbose {
        header.push_str(&format!(" {:<10} {:<20}", "pid", "cc-session"));
    }
    header.push_str(" worktree");
    println!("{}", header);
    println!("{}", "─".repeat(header.len().max(80)));
    // BUG-312: HLC-derived UUIDs put the timestamp at the start, so two
    // leases created in the same generation window share the historical
    // 8-char prefix. Disambiguate against the FULL active set (not just
    // `shown`) — `aida session end` resolves over the unfiltered set,
    // so the rendered id has to be unique against everything an end
    // call might match. trace:BUG-312 | ai:claude
    let all_ids: Vec<&str> = leases.iter().map(|l| l.id.as_str()).collect();
    for (l, state, pid) in &shown {
        let prefix_len = unique_prefix_len(&l.id, &all_ids, 8);
        let mut row = format!(
            "{:<14} {:<20} {:<18} {:<14}",
            (&l.id[..prefix_len]).yellow(),
            truncate(&l.scope, 20),
            truncate(&l.branch, 18),
            truncate(l.role.as_deref().unwrap_or("-"), 14),
        );
        if all {
            let label = format!("{} {}", state.glyph(), state.label());
            let colored = match state {
                LeaseState::Live => label.green(),
                LeaseState::Dormant => label.cyan(),
                LeaseState::Stale => label.yellow(),
            };
            row.push_str(&format!(" {:<10}", colored));
        }
        if verbose {
            let pid_col = pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
            let cc = cc_session_id_for_worktree(&l.worktree_path)
                .map(|s| s.chars().take(20).collect::<String>())
                .unwrap_or_else(|| "-".into());
            row.push_str(&format!(" {:<10} {:<20}", pid_col, cc));
        }
        row.push_str(&format!(" {}", l.worktree_path.display()));
        println!("{}", row);
        // STORY-301: annotate the lease the orchestrator is currently driving.
        if let Some((cur, phase)) = &drain_current {
            if &l.scope == cur {
                let phase_txt = phase.as_deref().unwrap_or("starting");
                println!(
                    "{}",
                    format!("{}◆ orchestrator drain — phase {phase_txt}", " ".repeat(15)).cyan()
                );
            }
        }
    }
    if !hidden_stale.is_empty() {
        println!();
        println!(
            "{}",
            format!(
                "({} stale lease{} hidden — pass --all to show)",
                hidden_stale.len(),
                if hidden_stale.len() == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }

    // STORY-69: --verbose warns about leaked claudes — processes whose
    // cwd is `(deleted)` (worktree was removed without ending claude
    // first, the BUG-61 signature). Note: live-in-lease claude PIDs
    // now appear inline in the `pid` column for each lease row
    // (TASK-56), so the previous "Live claude processes in lease
    // worktrees" sub-section was redundant and was removed.
    // trace:STORY-69 | ai:claude
    if verbose {
        let leaked: Vec<_> = live.iter().filter(|s| s.stale_cwd).collect();
        if !leaked.is_empty() {
            println!();
            println!("{}", "Leaked claude processes:".bold());
            for s in &leaked {
                println!(
                    "  {} pid {} cwd {} {} — `kill {}` to clean up",
                    "WARN".yellow().bold(),
                    s.pid.to_string().yellow(),
                    s.cwd.display(),
                    "(deleted)".dimmed(),
                    s.pid
                );
            }
        }
    }

    // STORY-637: surface cross-clone lease claims held by OTHER clones —
    // distinct from the local leases above. trace:STORY-637 | ai:claude
    print_cross_clone_leases(&project_root, now, all);

    println!();
    println!(
        "End one with: {} {}",
        "aida session end".cyan(),
        "<id>".dimmed()
    );
    Ok(())
}

/// One `.jsonl` file old enough to prune. Captured up-front so the
/// confirmation list and the deletion loop see the same set.
/// trace:STORY-60 | ai:claude
pub(crate) struct PruneCandidate {
    pub(crate) path: std::path::PathBuf,
    pub(crate) size: u64,
    pub(crate) age_seconds: u64,
}

/// Format a byte count with a human-readable unit (KB/MB/GB). One
/// decimal place so a 1.5 MB file doesn't read as "1 MB" or "1500000 B".
/// trace:STORY-60 | ai:claude
pub(crate) fn humanize_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{} B", bytes)
    } else if b < MB {
        format!("{:.1} KB", b / KB)
    } else if b < GB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.1} GB", b / GB)
    }
}

/// Compute the new queue position for `aida queue move X --after Y`.
/// `anchor` is Y's current position; `successor` is the position of the
/// next entry strictly after Y, or `None` when Y is at the bottom.
///
/// Returns a position that sorts strictly between `anchor` and the next
/// entry. Midpoint math when there's room; +1 fallback when adjacent
/// (collision risk acknowledged — safe in practice because positions are
/// initially gapped by 1000); +1000 when Y is at the bottom. All math is
/// saturating so a corrupt-state queue (every item at `i64::MAX` from a
/// pre-fix queue_add, see git_backend.rs) doesn't panic on overflow —
/// the callsite is expected to surface a friendlier hint instead.
/// trace:STORY-72 | ai:claude
pub(crate) fn position_after(anchor: i64, successor: Option<i64>) -> i64 {
    match successor {
        Some(np) if np > anchor.saturating_add(1) => {
            anchor.saturating_add((np.saturating_sub(anchor)) / 2)
        }
        Some(_) => anchor.saturating_add(1),
        None => anchor.saturating_add(1000),
    }
}

/// Compute the queue order after `aida queue move X --to N` — moving
/// `moved_id` to the 1-indexed absolute slot `requested`.
///
/// `entries` is the queue in display order (position-sorted) and must
/// include `moved_id`. The moved id is dropped, then re-inserted at the
/// clamped slot, so `--to 3` lands the item at the third slot of the
/// *final* queue regardless of where it started. `requested` is clamped
/// to `1..=entries.len()`: an out-of-range N moves the item to the
/// nearest end rather than erroring (the callsite surfaces a hint). The
/// caller renumbers the returned id list with the standard 1000-gap.
/// trace:TASK-280 | ai:claude
pub(crate) fn move_to_absolute_position(
    entries: &[uuid::Uuid],
    moved_id: uuid::Uuid,
    requested: usize,
) -> (Vec<uuid::Uuid>, usize) {
    let queue_len = entries.len();
    let mut order: Vec<uuid::Uuid> = entries.iter().copied().filter(|&e| e != moved_id).collect();
    let slot = requested.clamp(1, queue_len.max(1));
    let insert_idx = (slot - 1).min(order.len());
    order.insert(insert_idx, moved_id);
    (order, slot)
}

pub(crate) fn humanize_age_secs(secs: u64) -> String {
    if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else if secs < 7 * 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs < 30 * 86_400 {
        format!("{}w", secs / (7 * 86_400))
    } else if secs < 365 * 86_400 {
        format!("{}mo", secs / (30 * 86_400))
    } else {
        format!("{}y", secs / (365 * 86_400))
    }
}

/// `aida session prune` — walk Claude Code's per-project session
/// directories under `~/.claude/projects/<encoded>/` and delete `.jsonl`
/// files whose mtime is older than `days`. Skips any project dir
/// corresponding to an active session lease so a long-running session
/// doesn't self-delete from a forgotten cron-like usage. Logs each
/// deletion to `<project>/.aida/session-prune.log` for auditing.
/// trace:STORY-60 | ai:claude
/// Walk `~/.claude/projects/*` and find project dirs whose recorded cwd
/// no longer exists on disk. Each surviving jsonl encodes its cwd in
/// every event; we read one jsonl per project dir, extract the cwd
/// from the first event that has one, and check the filesystem. No
/// jsonls in the dir → treat as orphan (the dir is content-less).
/// Falls back to the lossy dir-name decode (replace `-` with `/`) when
/// the jsonl lacks a `"cwd":` field. trace:TASK-70 | ai:claude
/// TASK-358: mechanically tear down a lease + its worktree without the
/// interactive ceremony of [`session_end`] — no CI probe, no live-claude
/// refusal, no dirty-tree refusal, no prompts. Only callers that already
/// know the lease is safe to remove (e.g. the orchestrator's
/// `--escalate-blocks` marker [`SessionLease::escalated_to_human`]) should
/// reach this. Mirrors `session_end`'s interior writes — strip runtime
/// symlinks → unlink lease/manifest/activity log → `git worktree remove
/// --force` — and warns rather than errors on partial failure so a sweep
/// that hits one stuck worktree still tries the rest. Returns `true` when
/// the worktree was removed cleanly.
/// trace:TASK-358 | ai:claude
pub(crate) fn force_cleanup_lease(project_root: &std::path::Path, lease: &SessionLease) -> bool {
    // BUG-511: advisory leases (review-verb / MCP claims that recorded no
    // worktree) have nothing on disk beyond the lease file itself — and an
    // empty `worktree_path` would make the symlink-strip and `git worktree
    // remove` legs below resolve relative to CWD. Remove the lease file
    // (after the same activity aggregation) and stop.
    if lease.worktree_path.as_os_str().is_empty() {
        aggregate_session_activity_into_roles(project_root, &lease.id);
        let _ = std::fs::remove_file(lease_path(project_root, &lease.id));
        let activity = session_activity_path(project_root, &lease.id);
        if activity.exists() {
            let _ = std::fs::remove_file(&activity);
        }
        return true;
    }

    // Snapshot the lease's authoritative on-disk path before we strip the
    // worktree's symlinks (which can break the symlink chain). Mirrors
    // BUG-56's pattern in `session_end`.
    let lease_file_via_symlink = lease_path(project_root, &lease.id);
    let canonical_lease = lease_file_via_symlink.canonicalize().ok();

    // Aggregate the session's activity log into project-level role activity
    // before we delete the log — same ordering as `session_end`.
    aggregate_session_activity_into_roles(project_root, &lease.id);
    let activity_file = session_activity_path(project_root, &lease.id);
    let canonical_activity = activity_file.canonicalize().ok();
    let activity_target: std::path::PathBuf = canonical_activity
        .clone()
        .unwrap_or_else(|| activity_file.clone());

    // Strip the runtime symlinks `session_start` installed so `git
    // worktree remove` doesn't refuse on untracked files (BUG-52).
    let store_link = lease.worktree_path.join(".aida-store");
    if store_link.is_symlink() {
        let _ = std::fs::remove_file(&store_link);
    }
    let aida_dir = lease.worktree_path.join(".aida");
    for runtime in &[
        "sessions",
        "roles",
        "cache.db",
        "cache.db-shm",
        "cache.db-wal",
        "pgdata",
    ] {
        let p = aida_dir.join(runtime);
        if p.is_symlink() {
            let _ = std::fs::remove_file(&p);
        }
    }

    // Lease file first — once gone the session no longer counts as active.
    let lease_target: &std::path::Path = canonical_lease
        .as_deref()
        .unwrap_or(&lease_file_via_symlink);
    match std::fs::remove_file(lease_target) {
        Ok(_) | Err(_) => {}
    }

    // Companion files — quiet on missing.
    let manifest_target = session_manifest::manifest_path(project_root, &lease.id);
    if manifest_target.exists() {
        let _ = std::fs::remove_file(&manifest_target);
    }
    if activity_target.exists() {
        let _ = std::fs::remove_file(&activity_target);
    }

    // `git worktree remove --force` — `--force` because the dirty-tree
    // refusal in `session_end` is gated by interactive prompts; an
    // escalation cleanup never had a chance to commit, so we just remove.
    let res = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "worktree",
            "remove",
            "--force",
            "--", // trace:BUG-1622 | ai:claude
            lease.worktree_path.to_str().unwrap_or_default(),
        ])
        .output();
    let removed = matches!(&res, Ok(o) if o.status.success());
    if !removed {
        eprintln!(
            "  {} could not remove worktree {} — remove manually with \
             `git worktree remove --force {}`",
            "Warning:".yellow().bold(),
            lease.worktree_path.display(),
            lease.worktree_path.display(),
        );
    }
    removed
}

/// TASK-358: on a triage that takes a spec out of `NeedsAttention`, find any
/// orchestrator-escalated lease for it and clean up the lingering worktree.
/// The lease's `escalated_to_human` marker is the load-bearing gate: only
/// leases set by `--escalate-blocks` are touched, so an interactive user
/// session on the same spec stays put, and the advisor-resume path
/// (which never sets the marker) preserves its worktree as STORY-306
/// requires. Best-effort: a missing project root or zero matching leases
/// is a quiet no-op. trace:TASK-358 | ai:claude
pub(crate) fn cleanup_escalated_leases_for_spec(project_root: &std::path::Path, spec_id: &str) {
    let leases = list_leases(project_root);
    let mut cleaned = 0usize;
    for lease in leases {
        if lease.escalated_to_human.is_none() {
            continue;
        }
        if !lease.scope.eq_ignore_ascii_case(spec_id) {
            continue;
        }
        eprintln!(
            "  {} cleaning up escalated worktree for {}: {}",
            "→".dimmed(),
            spec_id.cyan(),
            lease.worktree_path.display().to_string().dimmed(),
        );
        force_cleanup_lease(project_root, &lease);
        cleaned += 1;
    }
    if cleaned > 0 {
        eprintln!(
            "  {} {} escalated worktree{} cleaned up",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            cleaned,
            if cleaned == 1 { "" } else { "s" },
        );
    }
}

/// TASK-1311: the per-row requeue affordance under a NeedsAttention spec in
/// `aida findings list`. It shows the one command that returns the spec to
/// flight and the status it lands in, and puts an open decision first.
/// STORY-1429: rendered from the same `requeue_preview` the interactive
/// `aida rework` loop shows, so the hint and the keystroke cannot disagree.
// trace:TASK-1311 trace:STORY-1429 | ai:claude
pub(crate) fn print_requeue_hint_row(
    r: &aida_core::Requirement,
    did: &str,
    store: Option<&aida_core::RequirementsStore>,
    storage: &Storage,
) {
    let route = queue_cmd::current_queue_route(storage, r.id);
    let preview = requeue::requeue_preview(
        r,
        store,
        route.as_deref(),
        requeue::caller_may_clear_escalation(),
    );
    println!(
        "  {:<20} {:<14} {}",
        "",
        "",
        format!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::SubArrow),
            requeue::requeue_hint(did, &preview)
        )
        .dimmed()
    );
}

/// EPIC-28: park a spec in `NeedsAttention` with a structured
/// [`aida_core::FailureReason`] so a batch drain can continue past a phase
/// failure rather than halting. Called from
/// `RealPhaseDriver::shelve_on_failure` (which the orchestrator's
/// `finish_failure` invokes for every shelvable phase failure).
///
/// Returns the populated `FailureReason` on success (the orchestrator
/// stamps it on `OrchestrationResult::shelved_reason`); returns `Ok(None)`
/// when the spec cannot be safely shelved (e.g. it is already in a
/// terminal status). Best-effort: ledger-write failures are logged but
/// never undo the status flip, mirroring `aida punt`.
/// trace:EPIC-28 | ai:claude
pub(crate) fn shelve_spec_on_failure(
    project_root: &std::path::Path,
    spec: &str,
    phase_slug: &str,
    phase_index: u8,
    kind_slug: &str,
    detail: &str,
    recovery_hint: &str,
) -> anyhow::Result<Option<aida_core::FailureReason>> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        // No distributed store at this root → nothing to update. The
        // orchestrator only runs in git-canonical projects today; this is
        // defensive for the legacy centralized-SQLite layout where
        // shelving is not supported.
        return Ok(None);
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    // trace:TASK-1468 | ai:claude
    let Some(mut req) = backend.get_requirement_unambiguous(spec)? else {
        // Spec not in the store — orchestrator was driving a stale id.
        // Best-effort: log and skip rather than crash the failure path.
        eprintln!(
            "  {} could not load spec {} for shelving — store has no matching id",
            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            spec,
        );
        return Ok(None);
    };
    let display_id = req.display_id();

    let now = chrono::Utc::now();
    let fr = aida_core::FailureReason {
        phase: phase_slug.to_string(),
        phase_index,
        kind: kind_slug.to_string(),
        detail: detail.to_string(),
        recovery_hint: if recovery_hint.is_empty() {
            None
        } else {
            Some(recovery_hint.to_string())
        },
        shelved_by: std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|r| !r.is_empty()),
        shelved_at: now,
    };

    // Honor the STORY-332 transition guard. The common case is
    // `InProgress → NeedsAttention` (orchestrator drove the spec through
    // phase 1 pickup, which set InProgress). Edge cases where the spec
    // never reached InProgress (a phase-0 setup failure) still record the
    // FailureReason so the operator sees it, but skip the status flip so
    // we don't fight the guard.
    shelve_status_flip(&mut req);
    req.failure_reason = Some(fr.clone());
    req.modified_at = now;
    backend.update_requirement(&req)?;

    // Best-effort ledger append + role activity record. A failure here
    // must not undo the spec update, which is the load-bearing part.
    if let Err(e) = punt::append_failure_to_ledger(project_root, &display_id, &fr) {
        eprintln!(
            "  {} could not write shelving record to .aida/punts.jsonl: {e}",
            "Note:".dimmed()
        );
    }
    record_role_activity(&display_id, "shelve");

    Ok(Some(fr))
}

/// The shelve's status flip: `NeedsAttention` when the STORY-332 transition
/// guard allows it (from In Progress or Done), otherwise no change, so a
/// terminal spec is never moved. Returns whether the status changed.
///
/// BUG-1632: the shelve is an automated write, so the flip is recorded in the
/// status history under [`aida_core::conflict::ORCHESTRATOR_SHELVE_AUTHOR`].
/// The BUG-1625 merge guard then keeps a terminal status another clone reached
/// meanwhile instead of letting this newer NeedsAttention win.
// trace:EPIC-28 trace:BUG-1632 | ai:claude
pub(crate) fn shelve_status_flip(req: &mut aida_core::Requirement) -> bool {
    if aida_core::forbidden_attention_transition(
        &req.status,
        &aida_core::RequirementStatus::NeedsAttention,
    )
    .is_some()
    {
        return false;
    }
    let from = req.status.clone();
    req.status = aida_core::RequirementStatus::NeedsAttention;
    aida_core::conflict::record_status_transition(
        req,
        aida_core::conflict::ORCHESTRATOR_SHELVE_AUTHOR,
        &from,
    );
    req.status != from
}

/// BUG-1637: the PR-open Done assertion's status write. Automated, and only
/// ever In Progress -> Done, so it never leaves a terminal status. Recorded
/// under [`aida_core::conflict::PR_OPEN_AUTHOR`]. Returns whether it moved.
// trace:BUG-1637 | ai:claude
pub(crate) fn pr_open_done_flip(req: &mut Requirement) -> bool {
    if !matches!(req.status, RequirementStatus::InProgress) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        req,
        RequirementStatus::Done,
        aida_core::conflict::PR_OPEN_AUTHOR,
    );
    true
}

/// BUG-1637: the Approved -> In Progress coherence bump shared by the
/// SubagentStart lease-take hook ([`aida_core::conflict::LEASE_TAKE_AUTHOR`])
/// and `aida session start` ([`aida_core::conflict::SESSION_START_AUTHOR`]).
/// Automated; any other source status (terminal included) is left alone.
/// Returns whether it moved.
// trace:BUG-1637 | ai:claude
pub(crate) fn approved_to_in_progress_bump(req: &mut Requirement, author: &str) -> bool {
    if !matches!(req.status, RequirementStatus::Approved) {
        return false;
    }
    aida_core::conflict::set_status_recorded(req, RequirementStatus::InProgress, author);
    req.modified_at = chrono::Utc::now();
    true
}

/// BUG-1637: the zen autopilot's Draft -> Approved write. It applies only to
/// a Draft (the status the approve-gate judged and the autopilot audit records
/// as the prior state), so it never leaves a terminal status. Recorded under
/// [`aida_core::conflict::ZEN_APPROVE_AUTHOR`]. Returns whether it moved.
// trace:BUG-1637 | ai:claude
pub(crate) fn zen_approve_flip(req: &mut Requirement) -> bool {
    if !matches!(req.status, RequirementStatus::Draft) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        req,
        RequirementStatus::Approved,
        aida_core::conflict::ZEN_APPROVE_AUTHOR,
    );
    req.modified_at = chrono::Utc::now();
    true
}

/// BUG-1637: the orchestrator's phase-1 pre-spawn bump (BUG-369). Re-checks
/// the source status on the copy read inside the atomic write, so a spec a
/// concurrent writer moved to Rejected or Completed is not bumped, and records
/// the move under [`aida_core::conflict::ORCHESTRATOR_PHASE1_AUTHOR`].
/// Returns whether it moved.
// trace:BUG-369 trace:BUG-1637 | ai:claude
pub(crate) fn phase1_status_bump(
    r: &mut Requirement,
    target: &RequirementStatus,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    if auto_complete_phase1_target_status(&r.status).as_ref() != Some(target) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        r,
        target.clone(),
        aida_core::conflict::ORCHESTRATOR_PHASE1_AUTHOR,
    );
    r.modified_at = now;
    true
}

/// BUG-1637: TASK-133's restore of the pre-bump status after a phase-1
/// failure. Automated: it never leaves a terminal status (a spec that reached
/// one meanwhile is no longer the spec phase 1 bumped) and records the move
/// under [`aida_core::conflict::ORCHESTRATOR_PHASE1_AUTHOR`]. Returns whether
/// it applied.
// trace:TASK-133 trace:BUG-1637 | ai:claude
pub(crate) fn phase1_status_restore(req: &mut Requirement, prior: &RequirementStatus) -> bool {
    if aida_core::conflict::is_terminal_status(&req.status) {
        return false;
    }
    aida_core::conflict::set_status_recorded(
        req,
        prior.clone(),
        aida_core::conflict::ORCHESTRATOR_PHASE1_AUTHOR,
    );
    true
}

/// BUG-1637: the whole TASK-133 restore write, run inside the atomic write:
/// restore the pre-bump status and clear the spurious shelve FailureReason, or
/// change nothing when [`phase1_status_restore`] refuses. Returns whether it
/// applied.
// trace:TASK-133 trace:BUG-1637 | ai:claude
pub(crate) fn apply_phase1_restore(r: &mut Requirement, prior: &RequirementStatus) -> bool {
    if !phase1_status_restore(r, prior) {
        return false;
    }
    // The shelve that ran on the failure path stamped a FailureReason; with no
    // lease and no work behind it that finding is spurious — clear it.
    r.failure_reason = None;
    r.modified_at = chrono::Utc::now();
    true
}

// BUG-1638: race-test seam for the self-reading status writers
// (`ensure_spec_done_after_pr`, `bump_spec_in_progress_at_lease_take`,
// `restore_phase1_status_on_lease_failure`). Each reads the spec, then writes
// it through `update_spec_atomically`; a test arms a one-shot hook that runs
// between the read and the write, so it can make a concurrent change there and
// pin that the write re-checks the copy read under the store lock. Compiled
// out of non-test builds. trace:BUG-1638 | ai:claude
#[cfg(test)]
pub(crate) type StatusWriteRaceHook = Box<dyn FnOnce(&std::path::Path)>;

#[cfg(test)]
thread_local! {
    pub(crate) static STATUS_WRITE_RACE_HOOK: std::cell::RefCell<Option<StatusWriteRaceHook>> =
        const { std::cell::RefCell::new(None) };
}

/// BUG-1638: arm the one-shot hook [`status_write_race_seam`] runs on this
/// thread. The hook receives the store root the writer resolved.
// trace:BUG-1638 | ai:claude
#[cfg(test)]
pub(crate) fn inject_status_write_race(hook: impl FnOnce(&std::path::Path) + 'static) {
    STATUS_WRITE_RACE_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
}

/// BUG-1638: called by each self-reading status writer between its read and
/// its atomic write. A no-op outside tests.
// trace:BUG-1638 | ai:claude
pub(crate) fn status_write_race_seam(store_root: &std::path::Path) {
    #[cfg(test)]
    {
        let hook = STATUS_WRITE_RACE_HOOK.with(|h| h.borrow_mut().take());
        if let Some(hook) = hook {
            hook(store_root);
        }
    }
    #[cfg(not(test))]
    let _ = store_root;
}

/// BUG-1638: what [`restore_phase1_status_on_lease_failure`] did, so the
/// orchestrator reports a restore only when one happened.
// trace:BUG-1638 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Phase1RestoreOutcome {
    /// The pre-bump status was written back.
    Restored,
    /// Nothing was written; the reason says why.
    Refused(String),
}

/// BUG-1638: the orchestrator's line for a phase-1 restore outcome. It says
/// "status restored" only for [`Phase1RestoreOutcome::Restored`]. BUG-1647:
/// statuses read as the CLI shows them ("Needs Attention"), the same form the
/// refusal reason uses.
// trace:BUG-1638 trace:BUG-1647 | ai:claude
pub(crate) fn phase1_restore_outcome_message(
    outcome: &Phase1RestoreOutcome,
    display_id: &str,
    prior: &RequirementStatus,
) -> String {
    match outcome {
        Phase1RestoreOutcome::Restored => format!(
            "phase-1 startup failed before acquiring a lease — status restored to {prior} \
             (re-queueable); no work was stranded"
        ),
        Phase1RestoreOutcome::Refused(reason) => format!(
            "phase-1 startup failed before acquiring a lease — {display_id}'s status was not \
             restored to {prior}: {reason}"
        ),
    }
}

/// BUG-1651: the stderr line the orchestrator prints for a phase-1 restore
/// outcome, or `None` under `--json`: both the restore and the refusal are
/// suppressed so JSON output stays machine-readable (BUG-1647).
// trace:BUG-1647 trace:BUG-1651 | ai:claude
pub(crate) fn phase1_restore_report_line(
    outcome: &Phase1RestoreOutcome,
    display_id: &str,
    prior: &RequirementStatus,
    json: bool,
) -> Option<String> {
    if json {
        return None;
    }
    let glyph = match outcome {
        Phase1RestoreOutcome::Restored => "↩".cyan(),
        Phase1RestoreOutcome::Refused(_) => crate::glyph(crate::glyphs::Glyph::Info).cyan(),
    };
    Some(format!(
        "  {glyph} {}",
        phase1_restore_outcome_message(outcome, display_id, prior)
    ))
}

/// TASK-133: undo the orchestrator parent's pre-spawn phase-1 status bump.
///
/// `prepare_auto_complete_phase1_status` flips a spec Approved/Planned/Draft →
/// InProgress *before* spawning the implementer child (BUG-369). When phase 1
/// then fails without the child ever acquiring a lease, no work happened — but
/// the spec is now InProgress and, because `finish_failure` shelves shelvable
/// phase failures, typically NeedsAttention with a `failure_reason`. That
/// strands a spec the operator should be able to re-queue: there is nothing to
/// triage, only a transient spawn / contention error.
///
/// This restores the captured pre-bump status and clears the (spurious)
/// `failure_reason` so the spec drops out of `aida findings list` and is
/// cleanly pickable again. Best-effort, like the shelve it compensates: a
/// store-write error is logged but never aborts the drain epilogue. Writes
/// through `CachedGitBackend` so the read-projection (`aida list`) reflects
/// the reset immediately rather than waiting for a stale-detection rebuild.
/// trace:TASK-133 | ai:claude
/// BUG-479: does ANY child-side evidence of real work exist for `spec`?
/// Pure decision over the three probed signals — a live/stale lease scoped to
/// the spec, an on-disk worktree for it, or unmerged commits on its branch.
/// Any one being present means the implementer child DID acquire a lease and do
/// work before exiting non-zero (e.g. a post-commit `/aida-pr` failure or a
/// headless session aborting after committing), so the parent's "no lease ⇒ no
/// work" assumption is wrong and the spec must stay shelved for triage rather
/// than be reset. trace:BUG-479 | ai:claude
pub(crate) fn child_side_work_exists(
    has_lease: bool,
    has_worktree: bool,
    has_unmerged_commits: bool,
) -> bool {
    has_lease || has_worktree || has_unmerged_commits
}

/// BUG-479: probe the real world for child-side work on `spec`, returning
/// `(has_lease, has_worktree, has_unmerged_commits)`.
///
/// - `has_lease`: a session lease whose scope matches the spec (case-insensitive
///   — the same scope-matching `aida session start`'s double-claim guard and the
///   implementer-prompt lease lookups use).
/// - `has_worktree`: such a lease records a worktree path that still exists on
///   disk (the worktree the child created when it acquired the lease).
/// - `has_unmerged_commits`: such a lease's branch has commits not yet on the
///   default branch (`git rev-list --count <default>..<branch>` > 0).
///
/// Conservative: a git probe that can't run leaves its signal `false`, but the
/// lease/worktree signals already cover the dominant case, and the caller treats
/// ANY of the three as "leave it shelved". trace:BUG-479 | ai:claude
pub(crate) fn probe_child_side_work_for_spec(
    project_root: &std::path::Path,
    spec: &str,
) -> (bool, bool, bool) {
    let matching: Vec<SessionLease> = list_leases(project_root)
        .into_iter()
        .filter(|l| l.scope.eq_ignore_ascii_case(spec))
        .collect();
    let has_lease = !matching.is_empty();
    let has_worktree = matching.iter().any(|l| l.worktree_path.exists());

    let has_unmerged_commits = match detect_default_branch_ref(project_root) {
        Some(default_ref) => matching
            .iter()
            .any(|l| branch_has_unmerged_commits(project_root, &default_ref, &l.branch)),
        None => false,
    };

    (has_lease, has_worktree, has_unmerged_commits)
}

/// BUG-479 (extracted for BUG-1716): does `branch` carry commits not yet on
/// `default_ref` (`git rev-list --count <default>..<branch>` > 0)?
/// Conservative: an empty/default branch name or a git probe that can't run
/// reads as `false`.
// trace:BUG-479 trace:BUG-1716 | ai:claude
pub(crate) fn branch_has_unmerged_commits(
    project_root: &std::path::Path,
    default_ref: &str,
    branch: &str,
) -> bool {
    if branch.is_empty() || branch == default_ref {
        return false;
    }
    let range = format!("{default_ref}..{branch}");
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-list", "--count", &range])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        })
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// BUG-1716: the session-lease evidence feeding the `Missing`-log arm of
/// [`empty_launch_decision`]. `None` — no lease exists for THIS session (the
/// child never claimed one), which routes the failure to the BUG-1629/
/// BUG-1769 lost-child recovery, not the empty-launch lane. `Some(has
/// commits)` — the child claimed its lease (the TASK-204 shape), and the
/// bool says whether its branch carries unmerged commits (real work the
/// empty-launch release must never orphan). Keyed to THIS session's lease
/// via the minted `--session-id`, never a scope-wide sweep — a sibling
/// attempt's PR branch must not veto classifying this launch.
// trace:BUG-1716 | ai:claude
pub(crate) fn orchestrated_session_lease_evidence(
    project_root: &std::path::Path,
    claude_session_id: &str,
) -> Option<bool> {
    let (_, branch, _, _) = find_orchestrated_lease(project_root, claude_session_id)?;
    let has_commits = detect_default_branch_ref(project_root)
        .map(|default_ref| branch_has_unmerged_commits(project_root, &default_ref, &branch))
        .unwrap_or(false);
    Some(has_commits)
}

pub(crate) fn restore_phase1_status_on_lease_failure(
    project_root: &std::path::Path,
    spec: &str,
    prior: &aida_core::RequirementStatus,
) -> anyhow::Result<Phase1RestoreOutcome> {
    // BUG-479: before resetting, probe child-side reality. The caller gates on
    // the PARENT-side `implementer_lease` field, which is only set after a CLEAN
    // child exit — so a child that acquired a lease + worktree + committed and
    // THEN exited non-zero (post-commit `/aida-pr` failure, aborted headless
    // session) reaches here with `implementer_lease == None` even though real
    // work is on disk. Resetting would clear the shelve + restore status while
    // the orphaned lease/worktree/commits remain, so the next pickup collides
    // (BUG-436/BUG-438 class). If ANY child-side work exists, leave it shelved
    // for triage. Conservative: any doubt → don't restore. trace:BUG-479 | ai:claude
    let (has_lease, has_worktree, has_unmerged_commits) =
        probe_child_side_work_for_spec(project_root, spec);
    // BUG-1638: every early exit is a refusal the caller reports, never a
    // silent Ok that reads as "restored". trace:BUG-1638 | ai:claude
    if child_side_work_exists(has_lease, has_worktree, has_unmerged_commits) {
        return Ok(Phase1RestoreOutcome::Refused(format!(
            "a lease, worktree or commits exist for {spec}, so it stays shelved for triage \
             (run `aida findings list`)"
        )));
    }

    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(Phase1RestoreOutcome::Refused(
            "no AIDA store was found for this project".to_string(),
        ));
    };
    let dispenser = load_dispenser(&store_path)?;
    let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;

    let gone = || Phase1RestoreOutcome::Refused(format!("{spec} no longer exists"));
    // trace:TASK-1468 | ai:claude
    let Some(req) = backend.get_requirement_unambiguous(spec)? else {
        return Ok(gone());
    };
    // BUG-1638: the race-test seam between the read and the write.
    // trace:BUG-1638 | ai:claude
    status_write_race_seam(&store_path);
    // BUG-1637: the restore and its terminal-status refusal run on the copy
    // read under the store lock; a refused restore writes nothing.
    // trace:BUG-1637 | ai:claude
    let mut applied = false;
    let mut seen = req.status.clone();
    let written = backend.update_spec_atomically(&req, |r| {
        seen = r.status.clone();
        applied = apply_phase1_restore(r, prior);
    })?;
    Ok(match (written, applied) {
        (None, _) => gone(),
        (Some(_), true) => Phase1RestoreOutcome::Restored,
        (Some(_), false) => Phase1RestoreOutcome::Refused(format!(
            "it is now {seen}, a final status, so it was left as is"
        )),
    })
}

/// TASK-358 tests — the `--escalate-blocks` worktree cleanup.
///
/// The wiring exercised here:
///   1. `mark_lease_escalated_to_human` stamps the timestamp on the lease
///      file and round-trips through TOML.
///   2. `cleanup_escalated_leases_for_spec` only touches leases that
///      (a) carry the marker AND (b) match the spec by scope. A bare
///      interactive lease on the same spec is ignored (preserves user work),
///      and an escalated lease for an unrelated spec is ignored (the triage
///      hook is per-spec).
///
/// trace:BUG-511 | ai:claude
#[cfg(test)]
#[path = "tests/bug_511_review_lease_tests.rs"]
mod bug_511_review_lease_tests;

// The per-spec liveness verdict (`aida status <spec>`) and the `aida why`
// STALLED surfacing. trace:STORY-694 trace:BUG-623 | ai:claude
#[cfg(test)]
#[path = "tests/story_694_spec_liveness_tests.rs"]
mod story_694_spec_liveness_tests;

// BUG-637: the spec-scoped CLAIM gates — pre-pickup refuse (the BUG-634
// duplicate-fanout fix) + pre-edit liveness (the EPIC-54-reject fix). Both gates
// are pure over the lease set plus an injected liveness predicate, so the matrix
// is testable without a live process or a lease dir.
// trace:BUG-637 | ai:claude
#[cfg(test)]
#[path = "tests/bug_637_spec_claim_tests.rs"]
mod bug_637_spec_claim_tests;

/// TASK-957: `aida claim` / `aida unclaim` — the advisor-fan-out claim that
/// closes the BUG-637 gap. A claim is a spec-scoped advisory lease
/// (`claim_verb = true`, no worktree, pid-based liveness) that the BUG-637 gates
/// pick up exactly like an AIDA-launched lease.
// trace:TASK-957 | ai:claude
#[cfg(test)]
#[path = "tests/task_957_claim_tests.rs"]
mod task_957_claim_tests;

// The global running-work table (`aida ps`). trace:STORY-696 | ai:claude
#[cfg(test)]
#[path = "tests/story_696_ps_tests.rs"]
mod story_696_ps_tests;

// `held_prs` derived from the verdict corpus, so a refusal recorded outside
// `aida review record` is still surfaced. trace:BUG-1773 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1773_corpus_held_prs_tests.rs"]
mod bug_1773_corpus_held_prs_tests;

// The merge chokepoint and the marker/label mirrors derived from the same
// verdict corpus, so a refusal from any producer holds the PR.
// trace:BUG-1774 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1774_corpus_merge_gate_tests.rs"]
mod bug_1774_corpus_merge_gate_tests;

// `aida ps` flags a live seat whose mail identity would fall back to the
// shell user. trace:TASK-1451 | ai:claude
#[cfg(test)]
#[path = "tests/task_1451_mail_identity_ps_tests.rs"]
mod task_1451_mail_identity_ps_tests;

// The pending-approval marker's priority over the heuristic classifier, plus
// `aida ps` / `aida awaiting` end-to-end rendering of a Blocked seat.
// trace:TASK-1454 | ai:claude
#[cfg(test)]
#[path = "tests/task_1454_pending_approval_tests.rs"]
mod task_1454_pending_approval_tests;

// The orphaned-In-Progress detection → `aida awaiting` mapping.
// trace:BUG-1523 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1523_orphaned_in_progress_mapping_tests.rs"]
mod bug_1523_orphaned_in_progress_mapping_tests;

// trace:BUG-1656 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1656_subagent_liveness_tests.rs"]
mod bug_1656_subagent_liveness_tests;

// trace:BUG-1680 | ai:antigravity
#[cfg(test)]
#[path = "tests/bug_1680_salvage_main_tests.rs"]
mod bug_1680_salvage_main_tests;

// trace:BUG-1941 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1941_ps_tests.rs"]
mod bug_1941_ps_tests;

// The three `aida ps` reporting defects: landed work advertised as stalled,
// guessed fan-out ownership, and fan-out rows wearing the parent's role.
// trace:BUG-1681 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1681_ps_reporting_tests.rs"]
mod bug_1681_ps_reporting_tests;

// BUG-1740: `possibly_subagent` must not be corroborated by the reporting
// session, and a harness lease pinned to the project root is not a fan-out.
// trace:BUG-1740 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1740_fanout_self_corroboration_tests.rs"]
mod bug_1740_fanout_self_corroboration_tests;

/// trace:TASK-358 | ai:claude
#[cfg(test)]
#[path = "tests/task_358_escalation_cleanup_tests.rs"]
mod task_358_escalation_cleanup_tests;

/// Best-effort load of the requirements store for read-only spec-id /
/// title / status lookups inside session-manifest rendering. Tries
/// distributed git-canonical mode first (the default), then falls back
/// to the legacy YAML/SQLite path. Returns None on any error — caller
/// renders with "(not found)" placeholders so missing data degrades
/// gracefully.
///
/// Pure with respect to `project_root`: BOTH resolution modes read only stores
/// reachable from that root, so this can never return another project's
/// requirements. No process CWD, no `REQ_DB_NAME`, no registry default — see
/// the legacy arm's comment for what went wrong when it did (BUG-1732).
// trace:STORY-98 trace:BUG-1732 | ai:claude
pub(crate) fn load_store_for_lookup(
    project_root: &std::path::Path,
) -> Option<aida_core::RequirementsStore> {
    // Git-canonical: walk up from project_root looking for an .aida/config.toml.
    if let Some(store_path) = detect_distributed_store_from(project_root) {
        if let Ok(backend) = aida_core::GitBackend::new(&store_path) {
            if let Ok(s) = aida_core::DatabaseBackend::load(&backend) {
                return Some(s);
            }
        }
    }
    // Legacy fallback for projects that haven't migrated. BUG-1732: this used
    // `determine_requirements_path(None)`, which probes for `requirements.db` /
    // `requirements.yaml` relative to the PROCESS CWD and honours an ambient
    // `REQ_DB_NAME` — both of which ignore the `project_root` every one of this
    // function's ~70 callers went to the trouble of resolving. On a host where
    // the cwd happens to hold an unrelated legacy store, a project-scoped
    // lookup silently returned that other project's requirements, which is how
    // `aida awaiting` came to report six In-Progress specs from a different
    // project in its orphaned-in-progress channel. BUG-1574's author already
    // documented this hazard and hand-bypassed the fallback for one call site
    // (see `unattended_git_mutation_refusal`); this makes the function itself
    // pure with respect to its argument so no caller has to remember.
    //
    // `resolve_requirements_path_in` is reused rather than reimplemented so the
    // probe ORDER (`requirements.db` before `requirements.yaml`) stays defined
    // in one place. Passing `project_root` as its cwd and `None` for both the
    // `-p` option and `REQ_DB_NAME` leaves no ambient input: on that arm the
    // resolver returns before it ever reads the registry, so the empty registry
    // path below is inert and is passed to make that unreadability explicit
    // rather than to imply the registry participates.
    // trace:BUG-1732 | ai:claude
    aida_core::resolve_requirements_path_in(project_root, None, None, std::path::Path::new(""))
        .ok()
        .map(Storage::new)
        .and_then(|s| s.load().ok())
}

/// Statusline cache freshness state. Two independent axes folded into a
/// single label by severity:
///   1. cache.db vs local orphan branch HEAD — `Stale` when they differ
///      (next read will rebuild the cache).
///   2. local orphan branch HEAD vs `origin/aida-store` — `Behind` when
///      local lags origin (run `aida db sync --pull`).
///      `Unknown` covers "can't tell about origin yet" — either no recent
///      fetch (STORY-79 hasn't run, or it's been >threshold) or no
///      `origin/aida-store` ref locally. `Fresh` is the all-good state and
///      doesn't render. trace:STORY-78 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheFreshness {
    Fresh,
    Stale,
    Behind,
    Unknown,
    /// No `.aida-store` attached in this worktree (e.g. a harness /
    /// general-purpose worktree with no orphan store checked out).
    /// Distinct from `Unknown` (origin freshness is transiently
    /// unknowable in an *attached* store): with no store at all there is
    /// nothing to report, so the segment is omitted rather than rendering
    /// a misleading `cache:?`. trace:BUG-518 | ai:claude
    NoStore,
}

impl CacheFreshness {
    /// One-letter label used by statusline. None means "don't render"
    /// (the Fresh case — boring, hide it; and the NoStore case — there is
    /// no store to report on, BUG-518). trace:STORY-78 | ai:claude
    pub(crate) fn label(self) -> Option<&'static str> {
        match self {
            CacheFreshness::Fresh => None,
            CacheFreshness::Stale => Some("stale"),
            CacheFreshness::Behind => Some("behind"),
            CacheFreshness::Unknown => Some("?"),
            // No store attached → omit the segment entirely. trace:BUG-518
            CacheFreshness::NoStore => None,
        }
    }
}

/// Decide which freshness state to render given pre-collected inputs.
/// Pure — no I/O, no env reads — so the precedence rules can be
/// exhaustively tested.
///
/// `local_behind_origin` is a tri-state:
///   - Some(true)  → origin has commits local doesn't (pull needed)
///   - Some(false) → local is equal-to or strictly-ahead-of origin
///     (push may be needed but no pull) — render Fresh
///   - None        → we couldn't determine direction (no recent fetch,
///     no origin ref, or rev-list lookup failed)
///
/// The "strictly ahead → Fresh" semantics keep us from nagging the user
/// to pull when they have unpushed local commits — `cache:behind` would
/// be misleading because there's nothing on origin to pull.
/// trace:STORY-78 | ai:claude
pub(crate) fn classify_cache_freshness(
    recorded_cache_sha: Option<&str>,
    local_sha: &str,
    local_behind_origin: Option<bool>,
    last_fetch_age_secs: Option<u64>,
    freshness_threshold_secs: u64,
) -> CacheFreshness {
    // No local store attached (e.g. a harness / general-purpose worktree
    // with no `.aida-store` checked out) → there is nothing to report.
    // Signal NoStore so the statusline omits the segment rather than
    // rendering a misleading `cache:?` (which means "store attached,
    // origin freshness transiently unknown"). trace:BUG-518 | ai:claude
    if local_sha.is_empty() {
        return CacheFreshness::NoStore;
    }
    // Axis 1: cache vs local. Stale wins over every other state because
    // the immediate consequence (slow next read while cache rebuilds) is
    // user-visible regardless of remote state.
    let cache_matches_local = recorded_cache_sha
        .map(|r| sha_prefix_match(r, local_sha))
        .unwrap_or(false);
    if !cache_matches_local {
        return CacheFreshness::Stale;
    }
    // Axis 2: local vs origin. Requires both a recent fetch AND a
    // direction signal — either missing collapses to Unknown rather
    // than false-fresh. STORY-79 is the producer of the last-fetch
    // timestamp; until it ships, last_fetch_age_secs is always None and
    // we render `?` whenever cache matches local. trace:STORY-79 | ai:claude
    let fetch_is_recent = last_fetch_age_secs
        .map(|age| age <= freshness_threshold_secs)
        .unwrap_or(false);
    if !fetch_is_recent {
        return CacheFreshness::Unknown;
    }
    match local_behind_origin {
        None => CacheFreshness::Unknown,
        Some(true) => CacheFreshness::Behind,
        Some(false) => CacheFreshness::Fresh,
    }
}

/// Decide whether `local_sha` is strictly behind `origin_sha` in the
/// orphan store at `store_path`. "Strictly behind" means: origin has at
/// least one commit not reachable from local. Returns None when git
/// can't answer (e.g. unknown commit, fork without merge-base).
///
/// `local == origin` short-circuits to Some(false). Strictly-ahead also
/// returns Some(false) — we explicitly do NOT report "behind" for the
/// have-unpushed-commits case. trace:STORY-78 | ai:claude
pub(crate) fn local_lags_origin(
    store_path: &std::path::Path,
    local_sha: &str,
    origin_sha: &str,
) -> Option<bool> {
    if sha_prefix_match(local_sha, origin_sha) {
        return Some(false);
    }
    let range = format!("{}..{}", local_sha, origin_sha);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(store_path)
        .args(["rev-list", "--count", &range])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let count: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(count > 0)
}

/// Read `~/.aida/cache/last-fetch.toml` and return the age (in seconds)
/// of the most recent successful fetch for `project_root`. Returns None
/// when the file is absent, the entry is missing, the last result was
/// an error, or the timestamp is unparseable — all of which collapse to
/// "Unknown" in the freshness classifier. trace:STORY-78 | ai:claude
pub(crate) fn read_last_fetch_age_secs(project_root: &std::path::Path) -> Option<u64> {
    let home = aida_home_dir()?;
    let toml_path = home.join(".aida/cache/last-fetch.toml");
    let raw = std::fs::read_to_string(&toml_path).ok()?;
    let value: toml::Value = toml::from_str(&raw).ok()?;
    // Schema (per STORY-79): top-level keys are project root absolute
    // paths; each entry is a table { last_fetch = "<rfc3339>", result =
    // "ok" | "error" }. Look up by canonical project_root path.
    let canon = project_root.canonicalize().ok()?;
    let key = canon.to_string_lossy();
    let entry = value.get(key.as_ref())?;
    let result = entry.get("result").and_then(|v| v.as_str()).unwrap_or("ok");
    if result != "ok" {
        return None;
    }
    let ts_str = entry.get("last_fetch").and_then(|v| v.as_str())?;
    let ts = chrono::DateTime::parse_from_rfc3339(ts_str).ok()?;
    let now = chrono::Utc::now();
    let age = now.signed_duration_since(ts.with_timezone(&chrono::Utc));
    let secs = age.num_seconds();
    if secs < 0 {
        Some(0)
    } else {
        Some(secs as u64)
    }
}

/// `git rev-parse origin/aida-store` against the orphan-store worktree.
/// Returns None when the ref doesn't exist (no remote configured, never
/// fetched) or the git invocation otherwise fails. The caller maps None
/// to `CacheFreshness::Unknown` rather than treating it as
/// "no-difference". trace:STORY-78 | ai:claude
pub(crate) fn rev_parse_origin_aida_store(project_root: &std::path::Path) -> Option<String> {
    let store_path = if project_root.join(".aida-store").exists() {
        project_root.join(".aida-store")
    } else if project_root.join("aida-store").exists() {
        project_root.join("aida-store")
    } else {
        return None;
    };
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&store_path)
        .args(["rev-parse", "--short", "origin/aida-store"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Threshold above which a last-fetch timestamp is treated as "stale"
/// (i.e. we render `cache:?` rather than `cache:fresh|behind`). Matches
/// STORY-79's default background-fetch interval so a single missed fetch
/// cycle is the boundary. Overridable via `AIDA_FETCH_FRESHNESS_SECS` so
/// CI / tests can dial it without hardcoding `300`. trace:STORY-78 | ai:claude
pub(crate) fn cache_freshness_threshold_secs() -> u64 {
    std::env::var("AIDA_FETCH_FRESHNESS_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
}

/// STORY-78 Part 2: the implementation of `--sync` on read commands.
/// Mirrors `aida db sync --pull` but without the commit-pending step
/// (read commands have no pending work to commit) and without conflict
/// detection (the caller is about to read the result anyway). Designed
/// to NEVER fail the caller: every failure mode is converted to a
/// warning and the command continues with the local view.
///
/// Failure modes handled here:
///   - store missing / not a git repo → warn, skip
///   - orphan store has uncommitted changes → warn, skip (pull --rebase
///     would abort partway and leave weirder state than we started with)
///   - offline / unreachable origin → warn, skip
///   - any other git error during rebase → abort the rebase, warn, skip
///
/// On success, updates `~/.aida/cache/last-fetch.toml` so the statusline
/// `cache:behind|?` indicator immediately reflects the pull (instead of
/// waiting for STORY-79's background fetcher).
/// trace:STORY-78 | ai:claude
pub(crate) fn maybe_sync_pull(store_path: &std::path::Path) -> Result<()> {
    if !aida_core::git_ops::is_git_repo(store_path) {
        eprintln!(
            "{} --sync: store at {} is not a git repo; skipping pull",
            "Warning:".yellow().bold(),
            store_path.display()
        );
        return Ok(());
    }
    // trace:TASK-1717 | ai:claude — the dirty check, the pull --rebase and
    // the clean-up of a rebase THIS pull started run under the store write
    // lock. A rebase that was already in progress (someone else's recovery)
    // is reported and left alone, never aborted.
    let pulled = aida_core::db::with_store_write_lock(store_path, || {
        // A failing `git status` is a failed pull, not a clean tree.
        match aida_core::git_ops::has_changes_checked(store_path) {
            Ok(true) => return Ok(SyncPull::Dirty),
            Ok(false) => {}
            Err(e) => return Ok(SyncPull::Failed(e)),
        }
        let branch = aida_core::git_ops::current_branch(store_path)
            .unwrap_or_else(|_| "aida-store".to_string());
        eprintln!("{} pulling origin/{} (--sync)", "→".dimmed(), branch);
        Ok(
            match aida_core::git_ops::pull_rebase(store_path, "origin", &branch) {
                Ok(()) => SyncPull::Pulled,
                Err(e) => {
                    // pull --rebase aborts the rebase it started; if that abort
                    // failed too, retry it once while still holding the lock —
                    // the rebase is ours, since none was in progress when the
                    // lock was taken. trace:STORY-78 | ai:claude
                    if aida_core::git_ops::rebase_in_progress(store_path) {
                        let _ = std::process::Command::new("git")
                            .arg("-C")
                            .arg(store_path)
                            .args(aida_core::git_ops::transaction_git_config())
                            .args(["rebase", "--abort"])
                            .output();
                    }
                    SyncPull::Failed(e)
                }
            },
        )
    });
    match pulled {
        Ok(SyncPull::Dirty) => {
            eprintln!(
                "{} --sync: orphan store has uncommitted changes; skipping pull",
                "Warning:".yellow().bold()
            );
            eprintln!("  Run `aida db sync --pull` manually after committing or stashing.");
        }
        Ok(SyncPull::Pulled) => {
            // Best-effort: refresh the statusline freshness signal so
            // the user doesn't see `cache:?` on the very next prompt
            // after they explicitly pulled. Silent on failure — losing
            // this write costs us a stale-looking indicator for one
            // render cycle, no worse.
            let _ = touch_last_fetch_ok(store_path);
            // TASK-1033: opportunistic store maintenance rides the sync —
            // ensure the lowered gc.auto is set, then `git gc --auto` (a cheap
            // no-op unless the threshold is exceeded). Best-effort.
            aida_core::git_ops::opportunistic_store_gc(store_path);
        }
        Ok(SyncPull::Failed(e)) | Err(e) => {
            let first_line = e
                .to_string()
                .lines()
                .next()
                .unwrap_or("unknown error")
                .to_string();
            eprintln!(
                "{} --sync pull failed: {}",
                "Warning:".yellow().bold(),
                first_line
            );
            eprintln!("  Falling back to local view.");
        }
    }
    Ok(())
}

/// Outcome of `maybe_sync_pull`'s locked window.
// trace:TASK-1717 | ai:claude
enum SyncPull {
    Dirty,
    Pulled,
    Failed(anyhow::Error),
}

/// Stamp `~/.aida/cache/last-fetch.toml` with "ok"/now for the project
/// whose orphan store is `store_path`. Best-effort — keys on the
/// canonicalized parent of `store_path` so reads (via
/// `read_last_fetch_age_secs(project_root)`) match writes.
/// trace:STORY-78 | ai:claude
pub(crate) fn touch_last_fetch_ok(store_path: &std::path::Path) -> Result<()> {
    write_last_fetch_entry(store_path, "ok")
}

/// Write a result string (`"ok"` for success, `"error: <msg>"` for
/// failure) plus a fresh timestamp into `~/.aida/cache/last-fetch.toml`
/// for the project whose orphan store is `store_path`. Shared between
/// the `--sync` path (STORY-78) and the background fetch worker
/// (STORY-79). trace:STORY-78 | ai:claude
pub(crate) fn write_last_fetch_entry(store_path: &std::path::Path, result: &str) -> Result<()> {
    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("store path has no parent"))?;
    let canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let home = aida_home_dir().context("no home dir")?;
    let cache_dir = home.join(".aida/cache");
    std::fs::create_dir_all(&cache_dir)?;
    let path = cache_dir.join("last-fetch.toml");

    let mut root: toml::value::Table = match std::fs::read_to_string(&path) {
        Ok(s) => toml::from_str::<toml::Value>(&s)
            .ok()
            .and_then(|v| v.as_table().cloned())
            .unwrap_or_default(),
        Err(_) => toml::value::Table::new(),
    };
    let mut entry = toml::value::Table::new();
    entry.insert(
        "last_fetch".into(),
        toml::Value::String(chrono::Utc::now().to_rfc3339()),
    );
    entry.insert("result".into(), toml::Value::String(result.into()));
    root.insert(
        canon.to_string_lossy().to_string(),
        toml::Value::Table(entry),
    );
    let serialized = toml::to_string(&toml::Value::Table(root))?;
    std::fs::write(&path, serialized)?;
    Ok(())
}

// ───────────────────────────────────────────────────────────────────
// STORY-79 — background fetch from statusline (auto-freshness)
// ───────────────────────────────────────────────────────────────────

/// Default seconds between background fetches. Statusline skips spawning
/// when the most recent fetch (or attempt) is within this window. Overridable
/// via `AIDA_BG_FETCH_INTERVAL_SECS`. trace:STORY-79 | ai:claude
pub(crate) fn bg_fetch_interval_secs() -> u64 {
    std::env::var("AIDA_BG_FETCH_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
}

/// Whether the background fetcher is enabled. `AIDA_BG_FETCH=false`
/// (or `0`, `no`, `off`) disables it entirely. Any other value (or
/// unset) leaves it enabled. trace:STORY-79 | ai:claude
pub(crate) fn bg_fetch_enabled() -> bool {
    match std::env::var("AIDA_BG_FETCH") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Path to the per-project lockfile used to coordinate background
/// fetches across concurrent shells. The basename mixes the project
/// dir name and a hex digest of the canonical path: dir name keeps it
/// debuggable, digest disambiguates two projects with the same
/// basename. trace:STORY-79 | ai:claude
pub(crate) fn bg_fetch_lock_path(store_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let project_root = store_path.parent()?;
    let canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let basename = project_root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into());
    // Cheap deterministic digest — we just need enough disambiguation
    // for human + scripted use, not collision resistance. FNV-1a 64-bit
    // by hand to avoid pulling in `siphasher`/`twox-hash`.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in canon.to_string_lossy().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let home = aida_home_dir()?;
    Some(
        home.join(".aida/cache")
            .join(format!(".bg-fetch-{}-{:016x}.lock", basename, h)),
    )
}

/// Best-effort: ensure `~/.aida/cache/` exists for the lock + toml writes.
/// Returns Ok even if creation fails — caller treats missing dir as
/// "skip background fetch this render". trace:STORY-79 | ai:claude
pub(crate) fn ensure_cache_dir() -> Option<std::path::PathBuf> {
    let home = aida_home_dir()?;
    let cache = home.join(".aida/cache");
    std::fs::create_dir_all(&cache).ok()?;
    Some(cache)
}

/// Should statusline kick off a fresh background fetch for this project?
/// Returns true when ALL conditions hold: feature enabled, no recent
/// fetch attempt (per `last-fetch.toml`), and no live lockfile.
/// Stale lockfiles (>3× interval) are considered dead and overridden.
/// Pure decision: callers do the lockfile create + spawn separately.
/// trace:STORY-79 | ai:claude
pub(crate) fn should_spawn_bg_fetch(
    last_fetch_age_secs: Option<u64>,
    lock_age_secs: Option<u64>,
    interval_secs: u64,
) -> bool {
    // Recent successful fetch (or attempt) means we're current.
    if matches!(last_fetch_age_secs, Some(age) if age < interval_secs) {
        return false;
    }
    // Live lockfile: another shell beat us to it. Treat as live for
    // 3× the interval to absorb slow git fetches on weak networks
    // without permanently wedging if a fetch crashed mid-flight.
    if matches!(lock_age_secs, Some(age) if age < interval_secs.saturating_mul(3)) {
        return false;
    }
    true
}

/// Spawn the `_bg-fetch` worker for `store_path` if the project is due
/// for a fetch and no other shell is already on it. All failure modes
/// (no home dir, can't create cache dir, lockfile-create race lost,
/// spawn failed) collapse to "skip silently" so statusline stays
/// silent and sub-50ms regardless. trace:STORY-79 | ai:claude
pub(crate) fn maybe_spawn_bg_fetch(project_root: &std::path::Path, store_path: &std::path::Path) {
    if !bg_fetch_enabled() {
        return;
    }
    let Some(_cache_dir) = ensure_cache_dir() else {
        return;
    };
    let Some(lock_path) = bg_fetch_lock_path(store_path) else {
        return;
    };
    let interval = bg_fetch_interval_secs();
    let last_fetch_age = read_last_fetch_age_secs(project_root);
    let lock_age = std::fs::metadata(&lock_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
        .map(|d| d.as_secs());
    if !should_spawn_bg_fetch(last_fetch_age, lock_age, interval) {
        return;
    }
    // Atomic claim: O_EXCL create. If another shell beat us between the
    // staleness check above and this call, the create fails and we
    // back off. trace:STORY-79 | ai:claude
    let claim = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path);
    let Ok(mut f) = claim else { return };
    use std::io::Write;
    let _ = writeln!(f, "{}", std::process::id());
    // Spawn ourselves as the hidden `_bg-fetch` worker, detached with
    // all stdio nulled. The current binary path comes from argv[0] when
    // it's absolute (cargo dev / installed); otherwise fall back to
    // `aida` on PATH. trace:STORY-79 | ai:claude
    let exe = aida_exe_path();
    let _ = std::process::Command::new(exe)
        .arg("_bg-fetch")
        .arg(store_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn_retrying_etxtbsy();
}

/// Pure source-label resolver for `aida whoami`'s user-id line. Mirrors
/// `current_user_id`'s resolution order (`AIDA_USER` → `USER` → `USERNAME` →
/// default) over already-read env values, so the printed source label can never
/// drift from the value `current_user_id` actually returned. Each arg is the
/// env var's value as `Option`, where an empty string counts as unset (matching
/// the `is_ok_and(|v| !v.is_empty())` checks the resolver uses).
/// trace:TASK-784
#[cfg(test)]
#[path = "tests/statusline_tests.rs"]
mod statusline_tests;

// trace:TASK-1479 | ai:claude
#[cfg(test)]
#[path = "tests/statusline_client_adapters_integration_tests.rs"]
mod statusline_client_adapters_integration_tests;

#[cfg(test)]
#[path = "tests/bug_88_pr_lookup_parse_tests.rs"]
mod bug_88_pr_lookup_parse_tests;

/// BUG-257: `gh_stderr_is_network_error` classifies a `gh` stderr line as a
/// transient network error vs. an auth/parse/other failure. The split powers
/// the `PrLookup::GhUnreachable` variant so the orchestrator can report
/// *Inconclusive* (drain pauses) instead of conflating "no PR" with "can't
/// reach the API". trace:BUG-257 | ai:claude
#[cfg(test)]
#[path = "tests/bug_257_gh_stderr_network_classifier_tests.rs"]
mod bug_257_gh_stderr_network_classifier_tests;

/// BUG-266: `claude_log_indicates_api_outage` classifies the headless
/// implementer's JSONL log as evidence the upstream Anthropic API took the
/// session out (a transient outage), rather than the implementer attempting
/// the work and failing. The orchestrator flips that phase from *failed* to
/// *Inconclusive* (the BUG-257 path), so a 529 / 5xx / stream-timeout no
/// longer marks a spec failed-by-implementer. trace:BUG-266 | ai:claude
#[cfg(test)]
#[path = "tests/bug_266_anthropic_api_outage_classifier_tests.rs"]
mod bug_266_anthropic_api_outage_classifier_tests;

#[cfg(test)]
#[path = "tests/bug_354_text_question_classifier_tests.rs"]
mod bug_354_text_question_classifier_tests;

#[cfg(test)]
#[path = "tests/bug_87_queue_filter_tests.rs"]
mod bug_87_queue_filter_tests;

/// BUG-231: `aida findings promote` flipped status to Approved and printed
/// "joins the work queue" but never called `queue add` — the finding ended
/// up Approved and in no queue (a silent half-success). These cover the
/// fixed `queue_promoted_finding` helper: it routes to a real queue, honors
/// `--for`, and surfaces a queue-add failure as an error.
#[cfg(test)]
#[path = "tests/bug_231_findings_promote_tests.rs"]
mod bug_231_findings_promote_tests;
#[cfg(test)]
#[path = "tests/cr_8_filing_provenance_tests.rs"]
mod cr_8_filing_provenance_tests;
#[cfg(test)]
#[path = "tests/story_1428_gate_promote_tests.rs"]
mod story_1428_gate_promote_tests;

/// BUG-574: reliability papercuts — spurious non-zero exits on the idempotent
/// re-entry paths of `aida session start` (re-running for a scope this clone
/// already leases) and `aida pr ship` (re-running on an already-merged PR).
/// Both are benign no-ops that completed, so they must exit 0 — reserving
/// non-zero for real failures — exactly like the BUG-573 read-only contract.
/// These tests pin the pure decision points that drive the exit-0 behavior.
/// trace:BUG-574 | ai:claude
#[cfg(test)]
#[path = "tests/bug_574_spurious_exit_tests.rs"]
mod bug_574_spurious_exit_tests;

#[cfg(test)]
#[path = "tests/lease_enforcement_tests.rs"]
mod lease_enforcement_tests;

#[cfg(test)]
#[path = "tests/scope_fallback_tests.rs"]
mod scope_fallback_tests;

#[cfg(test)]
#[path = "tests/store_walkup_tests.rs"]
mod store_walkup_tests;

#[cfg(test)]
#[path = "tests/session_end_resolution_tests.rs"]
mod session_end_resolution_tests;

#[cfg(test)]
#[path = "tests/terminal_status_guard_tests.rs"]
mod terminal_status_guard_tests;

/// TASK-292 — `aida queue work --auto-complete` with no positional SPEC id
/// inherits the no-arg "pick the queue head" semantics. These cover the pure
/// pickup-order resolver: drivable-status classification, skipping in-flight
/// heads, and the empty-vs-all-in-flight error split.
#[cfg(test)]
#[path = "tests/auto_complete_head_tests.rs"]
mod auto_complete_head_tests;

/// TASK-293 — the `next` / `nextN` keyword parser. Covers every named form
/// in the acceptance criteria (`next`, `next1`, `next 1`, `next3`, `next 3`)
/// plus the malformed-input edge cases. The drain machinery itself
/// (empty-queue, N > queue-length) is the already-tested `drain_batch` /
/// `pick_auto_complete_head` — this module only covers the pure parse.
/// trace:TASK-293 | ai:claude
#[cfg(test)]
#[path = "tests/next_keyword_tests.rs"]
mod next_keyword_tests;

/// STORY-86 — env-flag opt-out + colorize_status coverage for the new
/// `Done` variant. Integration coverage (set up git+store, exercise the
/// helper end-to-end) lives in the verification script in the plan; the
/// unit slice here protects the bits that are pure functions.
/// trace:STORY-86 | ai:claude
#[cfg(test)]
#[path = "tests/auto_bump_done_tests.rs"]
mod auto_bump_done_tests;

/// BUG-95: regression coverage for the claim that `aida pull --code-only`
/// skips the Done → Completed auto-bump. The bug-as-filed asserted the
/// conditional was nested in the wrong branch; the current structure has
/// the auto-bump correctly inside the code-pull block (gated by
/// `!store_only`), so `--code-only` (code_only=true, store_only=false)
/// reaches it. This test exercises that path end-to-end via
/// `handle_pull_command` and verifies the bump fires.
/// trace:BUG-95 | ai:claude
#[cfg(test)]
#[path = "tests/handle_pull_command_tests.rs"]
mod handle_pull_command_tests;

/// TASK-106: coverage for the `aida push` / `aida pull` scope flags —
/// all four combinations (default / --code-only / --store-only / both)
/// at the clap-parse layer, the `AIDA_PUSH_DEFAULT` env-var override,
/// and the mutual-exclusion guard. trace:TASK-106 | ai:claude
#[cfg(test)]
#[path = "tests/push_pull_scope_tests.rs"]
mod push_pull_scope_tests;

/// TASK-234 / TASK-241: integration coverage for the git-state
/// classifiers behind `aida queue list`'s "Done — awaiting merge"
/// section and `aida show`'s git-linkage section. Both extracted
/// helpers (`classify_in_flight_specs`, `collect_git_linkage`) are
/// gh-free — they only run `git` — so a temp repo with hand-built
/// commits exercises every branch without a GitHub round-trip.
/// trace:TASK-234 TASK-241 | ai:claude
#[cfg(test)]
#[path = "tests/in_flight_linkage_integration_tests.rs"]
mod in_flight_linkage_integration_tests;

/// TASK-241: coverage for the squash-merge PR-number parser that
/// `aida show`'s git-linkage section uses to point shipped specs at
/// their merged PR. trace:TASK-241 | ai:claude
#[cfg(test)]
#[path = "tests/git_linkage_tests.rs"]
mod git_linkage_tests;

/// STORY-511: coverage for the forge-aware change-request (PR/MR) linkage
/// formatter that `aida show`'s git-linkage section renders. Fully
/// isolated — no git repo, no `gh`/`glab` on PATH, no I/O — the rendering
/// is a pure function of (ForgeKind, ChangeLinkageState). EPIC-35 slice 5.
/// trace:STORY-511 | ai:claude
#[cfg(test)]
#[path = "tests/change_linkage_format_tests.rs"]
mod change_linkage_format_tests;

/// TASK-1475 (CR-8 acceptance 6 follow-up): coverage for `filing_drift_hint`
/// — the filing-provenance staleness signal `aida show` / `aida why`
/// render. Pure git, no `gh`/`glab` on PATH.
// trace:TASK-1475 | ai:claude
#[cfg(test)]
#[path = "tests/task_1475_filing_drift_tests.rs"]
mod task_1475_filing_drift_tests;

/// TASK-238: coverage for the queue-list tag surfacing — the inline
/// chip formatter and the `--by-batch` group key. trace:TASK-238
#[cfg(test)]
#[path = "tests/queue_tag_tests.rs"]
mod queue_tag_tests;

/// TASK-107: regression coverage for `aida fetch`. The behaviors we
/// pin: (1) code-leg fetch updates origin/<branch> without touching the
/// worktree, (2) cache-invalidation hook fires after the store leg
/// (best-effort, off the real `~/.aida` since `AIDA_HOME` overrides it
/// for tests), (3) `--code-only` / `--store-only` route correctly,
/// (4) the auto-bump does NOT fire on fetch — that's a pull-only
/// behavior. trace:TASK-107 | ai:claude
#[cfg(test)]
#[path = "tests/handle_fetch_command_tests.rs"]
mod handle_fetch_command_tests;

#[cfg(test)]
#[path = "tests/derive_parent_epic_label_tests.rs"]
mod derive_parent_epic_label_tests;

#[cfg(test)]
#[path = "tests/format_review_story_display_tests.rs"]
mod format_review_story_display_tests;

#[cfg(test)]
#[path = "tests/auto_branch_tests.rs"]
mod auto_branch_tests;

#[cfg(test)]
#[path = "tests/worktree_dirty_entries_tests.rs"]
mod worktree_dirty_entries_tests;

#[cfg(test)]
#[path = "tests/add_aida_gitignore_entries_tests.rs"]
mod add_aida_gitignore_entries_tests;

#[cfg(test)]
#[path = "tests/recent_files_for_branch_tests.rs"]
mod recent_files_for_branch_tests;

#[cfg(test)]
#[path = "tests/branch_behind_main_tests.rs"]
mod branch_behind_main_tests;

#[cfg(all(test, unix))]
mod resolve_gh_binary_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    // Serialize PATH-mutating tests against each other. Prepending (vs
    // replacing) PATH means other parallel tests keep finding `git` and
    // friends, but two concurrent PATH mutators still need to serialize
    // so they don't restore the wrong predecessor value. BUG-697: use the ONE
    // shared env lock so a PATH swap can't race any other env read. trace:BUG-79

    /// Acquire the PATH lock and prepend `dir` to PATH for the test's
    /// duration. Returns an RAII guard that restores PATH on drop.
    /// Prepending (instead of overwriting) keeps system `git` reachable
    /// from any parallel test that spawns subprocesses.
    // trace:TASK-1532 | ai:agy
    fn scoped_prepend_path(dir: &std::path::Path) -> impl Drop {
        crate::test_env::EnvVarGuard::prepend_path(dir)
    }

    fn make_executable(path: &std::path::Path) {
        crate::test_exec::write_executable(path, "#!/bin/sh\necho gh fake\n");
    }

    fn make_non_executable(path: &std::path::Path) {
        std::fs::write(path, "not executable\n").unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(path, perms).unwrap();
    }

    /// PATH walk picks up gh even when the system gh isn't on PATH.
    /// Critical regression guard for BUG-74. trace:BUG-74 | ai:claude
    #[test]
    fn finds_gh_via_path() {
        let tmp = TempDir::new().unwrap();
        let gh = tmp.path().join("gh");
        make_executable(&gh);
        let _g = scoped_prepend_path(tmp.path());
        let resolved = resolve_gh_binary().expect("expected to find fake gh on PATH");
        assert_eq!(
            std::fs::canonicalize(&resolved).unwrap(),
            std::fs::canonicalize(&gh).unwrap()
        );
    }

    /// PATH walk ignores non-executable files named gh. trace:BUG-74
    #[test]
    fn rejects_non_executable() {
        let tmp = TempDir::new().unwrap();
        let gh = tmp.path().join("gh");
        make_non_executable(&gh);
        let _g = scoped_prepend_path(tmp.path());
        let resolved = resolve_gh_binary();
        drop(_g);
        // We may still pick up a real system gh via the absolute-path
        // fallback — but we should NOT have picked up our broken fake.
        if let Some(p) = resolved {
            assert_ne!(
                std::fs::canonicalize(&p).unwrap(),
                std::fs::canonicalize(&gh).unwrap()
            );
        }
    }

    #[test]
    fn is_executable_checks_perm_bit() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("x");
        make_executable(&path);
        assert!(is_executable(&path));
        make_non_executable(&path);
        assert!(!is_executable(&path));
    }

    #[test]
    fn is_executable_rejects_dirs() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_executable(tmp.path()));
    }

    #[test]
    fn is_executable_rejects_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_executable(&tmp.path().join("nope")));
    }

    /// BUG-79: a file that passes is_executable but whose spawn fails
    /// (broken interpreter line) is not returned — we fall back to the
    /// next candidate or return None. trace:BUG-79 | ai:claude
    #[test]
    fn rejects_is_executable_but_spawn_fails() {
        let tmp = TempDir::new().unwrap();
        let bad = tmp.path().join("gh");
        // Shebang points at a non-existent interpreter, so spawn errors
        // with ENOENT even though is_executable returns true.
        crate::test_exec::write_executable(
            &bad,
            "#!/this/interpreter/does/not/exist\necho should never run\n",
        );
        assert!(is_executable(&bad));

        let _g = scoped_prepend_path(tmp.path());
        let resolved = resolve_gh_binary();
        drop(_g);
        // Either None (no other gh) or some other path — never the broken one.
        if let Some(p) = resolved {
            assert_ne!(
                std::fs::canonicalize(&p).unwrap(),
                std::fs::canonicalize(&bad).unwrap()
            );
        }
    }

    /// BUG-79: when PATH has a broken candidate followed by a working one,
    /// the working one wins. trace:BUG-79 | ai:claude
    #[test]
    fn falls_back_past_broken_to_working() {
        let broken_dir = TempDir::new().unwrap();
        let good_dir = TempDir::new().unwrap();

        let broken = broken_dir.path().join("gh");
        crate::test_exec::write_executable(&broken, "#!/this/does/not/exist\n");

        let good = good_dir.path().join("gh");
        make_executable(&good);

        // Prepend BOTH dirs so the broken one is hit first, then the good
        // one. Order matters: broken_dir wins on lookup order.
        let combined = std::path::PathBuf::from(format!(
            "{}:{}",
            broken_dir.path().display(),
            good_dir.path().display()
        ));
        let _g = scoped_prepend_path(&combined);
        let resolved = resolve_gh_binary().expect("should fall back to the working gh");
        drop(_g);
        assert_eq!(
            std::fs::canonicalize(&resolved).unwrap(),
            std::fs::canonicalize(&good).unwrap()
        );
    }
}

#[cfg(test)]
#[path = "tests/session_root_resolution_tests.rs"]
mod session_root_resolution_tests;

/// Apply the user's `--color=auto|always|never` choice to the colored
/// crate's global override. `auto` is the colored crate's default
/// behavior — it uses `isatty(stdout)` and respects `NO_COLOR`.
/// trace:FR-1-041 | ai:claude
pub(crate) fn apply_color_mode(mode: &str) {
    match mode {
        "always" => colored::control::set_override(true),
        "never" => colored::control::set_override(false),
        _ => {} // "auto" — let colored crate decide via tty detection
    }
}

/// Count queue entries in the user's queue file that pass the role filter.
/// When `role` is `Some`, returns the count of entries with `for_role`
/// matching exactly. When `role` is `None`, returns the total entry
/// count (no role filter).
///
/// Reads `<project>/.aida-store/registry/queues/<user>.yaml` directly —
/// keeps statusline off the heavier Storage::load() path so the sub-50ms
/// budget holds even when the orphan store has hundreds of objects.
/// Returns `None` if the file is missing or unreadable.
/// trace:FR-1-041 | ai:claude
/// TASK-648 (ADR-3): non-archived, non-deferred draft count from the cache,
/// read-only and fast (same SQLite-without-Cache::open pattern as the
/// statusline freshness probe — no migration, no write lock on the prompt hot
/// path). The advisor's statusline shows this as the active draft backlog to
/// triage. Returns 0 when the cache is absent or unreadable rather than
/// erroring the prompt.
// trace:BUG-1171 | ai:codex
pub(crate) fn read_draft_backlog_depth(cache_path: &std::path::Path) -> usize {
    if !cache_path.exists() {
        return 0;
    }
    let conn = match rusqlite::Connection::open_with_flags(
        cache_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let has_deferred_column = conn
        .prepare("SELECT deferred FROM requirements_cache LIMIT 0")
        .is_ok();
    let sql = if has_deferred_column {
        "SELECT COUNT(*) FROM requirements_cache \
         WHERE archived = 0 AND LOWER(status) = 'draft' AND deferred = 0"
    } else {
        "SELECT COUNT(*) FROM requirements_cache \
         WHERE archived = 0 AND LOWER(status) = 'draft'"
    };
    conn.query_row(sql, [], |row| row.get::<_, i64>(0))
        .map(|n| n.max(0) as usize)
        .unwrap_or(0)
}

/// Count URGENT unread mailbox messages for the current shell user, for the
/// statusline nag. Returns `None` when there is no mailbox to count (stay
/// silent), `Some(0)` when caught up.
///
/// This MUST agree with `aida mailbox inbox` — the statusline count and the read
/// command are two surfaces of one "unread" state, so reading the inbox has to
/// drive the count to 0. The divergence that left a high count stuck forever was
/// that this read only the LOCAL `.aida/mailbox/` layer while `aida mailbox
/// inbox` reads the MERGED (local + canonical orphan-store) set and advances each
/// identity's watermark to the MERGED per-identity newest. When the two message
/// sets differed, the watermark the inbox-read advanced could not clear a count
/// computed over a different set. The fix reads the SAME merged set and the same
/// identity union, so the surfaces compute "unread" over identical inputs and a
/// read clears the count. Reading the canonical layer is a plain directory read
/// of the already-checked-out `.aida-store/mailbox/` — pure file I/O, no git
/// spawn — so the statusline's no-git contract still holds.
// trace:STORY-539 trace:BUG-625 | ai:claude
pub(crate) fn read_urgent_unread_count(project_root: &std::path::Path) -> Option<usize> {
    let local = mailbox_store::read_local_messages(project_root).ok()?;
    // Merge in the canonical (orphan-store) layer so this sees exactly what
    // `aida mailbox inbox` reads (and advances watermarks against). Best-effort:
    // a missing/unreadable canonical dir degrades to the local layer rather than
    // silencing the nag. trace:BUG-625
    let store_root = project_root.join(".aida-store");
    let canonical = mailbox_store::read_canonical_messages(&store_root).unwrap_or_default();
    let merged = aida_core::mailbox::merge_dedup(&local, &canonical);
    if merged.is_empty() {
        return None;
    }
    // Scope to the same identity union the agent-facing notice and the inbox-read
    // use (shell user + session role + agent type), so a role-/type-addressed
    // urgent message isn't invisible here while the notice surfaces it.
    // build_notice dedups a broadcast across identities and counts urgent across
    // the unread set. trace:STORY-585 trace:BUG-625
    let identities = inbox_identities();
    let watermarks = mailbox_store::read_all_watermarks(project_root).ok()?;
    let summary = aida_core::mailbox::build_notice(
        identities.iter().map(String::as_str),
        &merged,
        &watermarks,
        aida_core::mailbox::NOTICE_DEFAULT_CAP,
    );
    Some(summary.urgent)
}

// ─── STORY-127: Scope-B runtime anti-pattern detectors ──────────────────
//
// Each surface the user TYPES (`aida pull`, `git pull` (via the code leg),
// `aida dev release`, `aida session end`) detects when its precondition is
// not met and emits a one-line WARNING (never a block — the user/agent
// always proceeds). The detection is a single git/queue/lease query.
//
// The detection LOGIC is a set of pure, unit-tested predicates so the
// warning conditions are testable without process/git/forge I/O. The call
// sites do the cheap query and feed the result to the predicate.
// trace:STORY-127 | ai:claude

/// Detector (1) + (2): `aida pull` / `git pull` (code leg) no-op.
///
/// `pull_was_noop` — the code-leg `git pull --ff-only` advanced nothing
/// (local branch already at origin). When that coincides with the user
/// holding an active **reviewer** lease on an **unmerged** PR, the no-op is
/// almost certainly the PR-27-style mistake: running catch-up commands
/// BEFORE the merge has happened. Warn so the user waits for the merge.
///
/// When there is no such reviewer lease the no-op is unremarkable — a plain
/// one-line note still helps ("Already up to date — nothing to pull") but is
/// not the same alarm. The two callers distinguish via the second arg.
/// trace:STORY-127 | ai:claude
pub(crate) fn pull_noop_warning(
    pull_was_noop: bool,
    reviewer_unmerged_pr: Option<&str>,
) -> Option<String> {
    if !pull_was_noop {
        return None;
    }
    match reviewer_unmerged_pr {
        Some(pr) => Some(format!(
            "No new commits on origin. You hold a reviewer lease on {pr}, which is \
             not merged yet — did you mean to wait for {pr} to merge first? \
             (catch-up before the merge is a no-op)"
        )),
        None => Some("Already up to date — origin had no new commits.".to_string()),
    }
}

/// Detector (2): cheap "local main already at origin/main" check, expressed
/// as a pure predicate over the two SHAs so it is testable. `None` for
/// either SHA (couldn't resolve a ref) means "can't tell" → no warning.
/// trace:STORY-127 | ai:claude
pub(crate) fn local_main_already_at_origin(
    local_sha: Option<&str>,
    origin_sha: Option<&str>,
) -> bool {
    match (local_sha, origin_sha) {
        (Some(a), Some(b)) => !a.is_empty() && a == b,
        _ => false,
    }
}

/// Detector (3): `aida dev release` (→ `scripts/release.sh`) with unmerged
/// PRs. Returns a one-line warning naming every open PR — a release tag cut
/// now will NOT include their changes. Pure over the already-collected
/// open-PR numbers. trace:STORY-127 | ai:claude
pub(crate) fn release_unmerged_pr_warning(open_pr_numbers: &[u64]) -> Option<String> {
    if open_pr_numbers.is_empty() {
        return None;
    }
    let mut nums: Vec<u64> = open_pr_numbers.to_vec();
    nums.sort_unstable();
    nums.dedup();
    let list = nums
        .iter()
        .map(|n| format!("PR-{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    let (verb, subj) = if nums.len() == 1 {
        ("is", "PR")
    } else {
        ("are", "PRs")
    };
    Some(format!(
        "{} open {} ({}) — the release {} not merged, so the tag will NOT include \
         their changes. Merge first if they belong in this release.",
        nums.len(),
        subj,
        list,
        verb,
    ))
}

/// Detector (4): `aida session end` while a DIFFERENT role has queued work
/// waiting. `ending_role` is the role of the lease being torn down;
/// `role_counts` is the per-role queue depth for the current user. Returns
/// the `(role, count)` pairs for every OTHER role with at least one waiting
/// item, sorted by role name for stable output. Pure over the inputs.
/// trace:STORY-127 | ai:claude
pub(crate) fn cross_role_queue_waiting(
    ending_role: Option<&str>,
    role_counts: &std::collections::HashMap<String, usize>,
) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = role_counts
        .iter()
        .filter(|(role, &count)| {
            count > 0 && ending_role.map(|er| er != role.as_str()).unwrap_or(true)
        })
        .map(|(role, &count)| (role.clone(), count))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Render the detector-(4) warning body from the cross-role pairs. `None`
/// when nothing is waiting elsewhere. trace:STORY-127 | ai:claude
pub(crate) fn session_end_cross_role_warning(waiting: &[(String, usize)]) -> Option<String> {
    if waiting.is_empty() {
        return None;
    }
    let list = waiting
        .iter()
        .map(|(role, count)| {
            format!(
                "{} has {} item{}",
                role,
                count,
                if *count == 1 { "" } else { "s" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "About to end this session, but another role's queue has work waiting \
         ({list}). Switch role first (`aida role enter <role>`) if you meant to \
         pick that up."
    ))
}

/// STORY-127: per-role queue depth for the current user. Reads the same
/// queue YAML `aida queue list` / `read_queue_depth` consult (keyed off
/// `current_user_id`). Returns an empty map on any read/parse failure —
/// the detectors degrade to silent. trace:STORY-127 | ai:claude
pub(crate) fn read_queue_role_counts(
    project_root: &std::path::Path,
) -> std::collections::HashMap<String, usize> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let user = current_user_id(None);
    let queue_path = project_root
        .join(".aida-store/registry/queues")
        .join(format!("{}.yaml", user));
    let Ok(content) = std::fs::read_to_string(&queue_path) else {
        return counts;
    };
    let Ok(entries) = serde_yaml::from_str::<Vec<serde_yaml::Value>>(&content) else {
        return counts;
    };
    for e in &entries {
        let role = e
            .get("for_role")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or("implementer");
        *counts.entry(role.to_string()).or_insert(0) += 1;
    }
    counts
}

/// STORY-127: does the current user hold an active **reviewer** lease whose
/// scope is a PR (`PR-N` / `MR-N`) that is still open (unmerged)? Returns
/// the PR label for the warning, else `None`. Cheap: walks the local lease
/// set, then a single forge open-PR snapshot keyed by number. The PR is
/// "unmerged" iff its number appears in the open-PR snapshot.
/// trace:STORY-127 | ai:claude
pub(crate) fn active_reviewer_unmerged_pr(project_root: &std::path::Path) -> Option<String> {
    let leases = list_leases(project_root);
    // Reviewer leases scope to a PR/MR; the scope string is e.g. "PR-27".
    let reviewer_pr_scopes: Vec<String> = leases
        .iter()
        .filter(|l| {
            l.role
                .as_deref()
                .map(|r| canonical_role_name(r) == "reviewer")
                .unwrap_or(false)
        })
        .filter(|l| pr_number_from_scope(&l.scope).is_some())
        .map(|l| l.scope.clone())
        .collect();
    if reviewer_pr_scopes.is_empty() {
        return None;
    }
    let open_numbers: std::collections::HashSet<u64> = collect_open_prs(project_root)
        .by_branch
        .values()
        .map(|p| p.number)
        .collect();
    for scope in reviewer_pr_scopes {
        if let Some(n) = pr_number_from_scope(&scope) {
            if open_numbers.contains(&n) {
                return Some(format!("PR-{n}"));
            }
        }
    }
    None
}

/// Parse a PR/MR number out of a lease scope like `PR-27` / `MR-3` / `pr-27`.
/// trace:STORY-127 | ai:claude
pub(crate) fn pr_number_from_scope(scope: &str) -> Option<u64> {
    let s = scope.trim();
    let rest = s
        .strip_prefix("PR-")
        .or_else(|| s.strip_prefix("pr-"))
        .or_else(|| s.strip_prefix("MR-"))
        .or_else(|| s.strip_prefix("mr-"))?;
    rest.parse::<u64>().ok()
}

pub(crate) fn read_queue_depth(
    project_root: &std::path::Path,
    role: Option<&str>,
) -> Option<usize> {
    // BUG-89 + BUG-675: resolve identity through the SAME two-step path
    // `aida queue list` uses — `current_user_id` first, then the queue-file
    // case-fold (`resolve_queue_user`, TASK-951/TASK-845) — so a shell reporting
    // `Joe` counts the queue keyed under `joe.yaml` instead of reading zero.
    // Reading `<current_user_id>.yaml` directly skipped that fold, so the depth
    // diverged from the queue-list count on a case-only identity mismatch; the
    // BUG-670 band-aid then suppressed the divergent number instead of fixing the
    // resolution. trace:BUG-675 trace:BUG-89 | ai:claude
    let store_path = project_root.join(".aida-store");
    let user = aida_core::db::resolve_queue_user(&store_path, &current_user_id(None));

    let queue_path = store_path
        .join("registry/queues")
        .join(format!("{}.yaml", user));

    // An absent own-queue file is zero own items, NOT "unknown" — a peer may
    // still have routed work to this role, and bailing here would hide it.
    // A file that exists but won't parse stays `None` (a real read failure).
    // trace:BUG-774 | ai:claude
    let entries: Vec<serde_yaml::Value> = match std::fs::read_to_string(&queue_path) {
        Ok(content) if content.trim().is_empty() => Vec::new(),
        Ok(content) => serde_yaml::from_str(&content).ok()?,
        Err(_) => Vec::new(),
    };

    let count = match role {
        Some(want) => entries
            .iter()
            .filter(|e| {
                e.get("for_role")
                    .and_then(serde_yaml::Value::as_str)
                    .map(|r| r == want)
                    .unwrap_or(false)
            })
            .count(),
        None => entries.len(),
    };
    // A `--for <role>` routing written by a PEER lands in that peer's queue
    // file, so a depth read scoped to our own file under-counted the work
    // actually routed to us — the meter disagreed with `aida queue list`.
    // Add the sibling files' entries routed to this role. Read-only; the
    // stored keys are untouched and identity folds through the shared
    // canonical helper. trace:BUG-774 | ai:claude
    let cross_user = match role {
        Some(want) => cross_user_role_routed_depth(&store_path, &user, want),
        None => 0,
    };
    Some(count + cross_user)
}

/// Count entries routed to `role` that live in OTHER users' queue files.
/// Best-effort + allocation-light (the same raw-YAML read the own-file depth
/// uses); an unreadable sibling file contributes zero rather than failing the
/// meter.
// trace:BUG-774 | ai:claude
pub(crate) fn cross_user_role_routed_depth(
    store_path: &std::path::Path,
    self_user: &str,
    role: &str,
) -> usize {
    let dir = store_path.join("registry/queues");
    let Ok(read) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let me = aida_core::node::canonical_user_id(self_user);
    let mut total = 0usize;
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if aida_core::node::canonical_user_id(stem) == me {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(rows) = serde_yaml::from_str::<Vec<serde_yaml::Value>>(&content) else {
            continue;
        };
        total += rows
            .iter()
            .filter(|e| {
                e.get("for_role")
                    .and_then(serde_yaml::Value::as_str)
                    .map(|r| r == role)
                    .unwrap_or(false)
            })
            .count();
    }
    total
}

/// TASK-647 (ADR-3): a status whose *production* requires advisor authority —
/// anything that places a spec into the active execution pipeline (Approved
/// and beyond). Draft / Rejected / NeedsAttention don't gate (they are not
/// "approved work" a non-advisor could smuggle past triage).
pub(crate) fn status_requires_advisor_authority(status: &RequirementStatus) -> bool {
    // TASK-739: single-sourced in the lifecycle model — the target half of the
    // advisor-authority predicate. trace:TASK-739 | ai:claude
    aida_core::lifecycle::target_requires_advisor_authority(
        aida_core::lifecycle::State::from_status(status),
    )
}

/// BUG-482: whether advancing a spec from `from` to `to` (via `aida edit
/// --status`) is an advisor-authority act keyed on the (source, target) pair.
///
/// A non-advisor may not lift an **un-triaged** (`Draft`) *or* a **punted**
/// (`NeedsAttention`) spec into the approved+ pipeline — both sources sit
/// awaiting the advisor's (or interactive human's) triage decision.
/// `forbidden_attention_transition` *permits* `NeedsAttention → Approved` as a
/// triage outcome, but *who* may make that call is still advisor authority;
/// before BUG-482 only `Draft` was a gated source, so a non-advisor could
/// self-re-approve a spec it (or the orchestrator) had just punted, bypassing
/// the triage the punt exists to request. Execution flips from a source
/// already in the pipeline (`Approved → InProgress → Done`) are NOT gated, so
/// drains are unaffected. trace:BUG-482 | ai:claude
pub(crate) fn status_advance_requires_advisor_authority(
    from: &RequirementStatus,
    to: &RequirementStatus,
) -> bool {
    // TASK-739: delegate to the lifecycle model's transition guard — the single
    // source for which (from → to) edits are advisor-authority acts. Defined
    // over the full domain (direct edits too, not only declared edges).
    // trace:TASK-739 | ai:claude
    use aida_core::lifecycle::{transition_guard, GuardKind, State};
    transition_guard(State::from_status(from), State::from_status(to))
        == GuardKind::RequiresAdvisorAuthority
}

/// BUG-1611: why `aida queue done` refuses a spec on lifecycle grounds.
/// `queue done` is an execution flip: its legal predecessors are the states a
/// spec reaches only after passing the approval gate (`Approved`, `Planned`,
/// `InProgress`, and an idempotent `Done`). Anything else is refused through
/// the existing lifecycle guard rather than a new rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueDoneLifecycleRefusal {
    /// `Draft` / `NeedsAttention`: never approved, or punted back for triage.
    /// The lifecycle guard marks `→ Done` from these as an advisor-authority
    /// act, and the caller does not hold that authority.
    NotApproved,
    /// `Rejected` / `Completed` / `Superseded`: closed. `queue done` is not a
    /// reopen verb, so it refuses whatever the caller's authority.
    Closed,
}

/// BUG-1611: the lifecycle/authority decision for `aida queue done`, shared by
/// the CLI handler and the MCP `queue_done` tool so the two surfaces cannot
/// drift. `has_advisor_authority` is the caller's resolved authority (CLI:
/// [`has_advisor_authority`]; MCP: the server's role). `None` = proceed.
/// `--force` deliberately has no say here.
// trace:BUG-1611 | ai:claude
pub(crate) fn queue_done_lifecycle_refusal(
    from: &RequirementStatus,
    has_advisor_authority: bool,
) -> Option<QueueDoneLifecycleRefusal> {
    if is_terminal_status(from) {
        return Some(QueueDoneLifecycleRefusal::Closed);
    }
    if status_advance_requires_advisor_authority(from, &RequirementStatus::Done)
        && !has_advisor_authority
    {
        return Some(QueueDoneLifecycleRefusal::NotApproved);
    }
    None
}

/// BUG-1611: the operator-facing refusal line for
/// [`queue_done_lifecycle_refusal`]. Neutral guidance only: it names the
/// legitimate route (approval, or a reopen that is itself authority-gated) and
/// never suggests an environment-variable role override.
// trace:BUG-1611 | ai:claude
pub(crate) fn queue_done_lifecycle_refusal_message(
    display_id: &str,
    from: &RequirementStatus,
    refusal: QueueDoneLifecycleRefusal,
) -> String {
    match refusal {
        QueueDoneLifecycleRefusal::NotApproved => format!(
            "queue done refused: {display_id} is {from} and has not passed the approval \
             gate, so marking it done would skip triage. Ask an advisor to approve it \
             (`aida edit {display_id} --status approved`), then work it and mark it done."
        ),
        QueueDoneLifecycleRefusal::Closed => format!(
            "queue done refused: {display_id} is already {from} (closed), and marking it \
             done would reopen it. To redo the work, have an advisor reopen it \
             (`aida edit {display_id} --status approved --force`) or file a new spec."
        ),
    }
}

/// BUG-1611: whether `aida zen`'s autopilot approve-gate may flip this spec to
/// `Approved`. `[autopilot] approve = "auto"` is a policy knob in an
/// agent-writable config file, so it cannot stand in for approval authority:
/// the auto-approve runs the same (source → Approved) lifecycle guard every
/// other approval path runs.
// trace:BUG-1611 | ai:claude
pub(crate) fn zen_auto_approve_authorized(
    from: &RequirementStatus,
    has_advisor_authority: bool,
) -> bool {
    !status_advance_requires_advisor_authority(from, &RequirementStatus::Approved)
        || has_advisor_authority
}

/// A manual `aida edit <epic> --status <X>` is forbidden — an epic's status is a
/// read-only rollup of its children. This generalizes the earlier "epics cannot
/// be promoted to Approved" rule to EVERY transition; `--force` is the recovery
/// escape.
// trace:BUG-626 trace:TASK-761 | ai:claude
pub(crate) fn manual_epic_status_edit_forbidden(req_type: &RequirementType, force: bool) -> bool {
    *req_type == RequirementType::Epic && !force
}

pub(crate) fn approval_forbidden_for_type(req_type: &RequirementType) -> bool {
    // trace:TASK-761 | ai:codex
    //
    // BUG-751: `Decision` is deliberately NOT in this set. A decision spec
    // (ADR) is stateful — its documented lifecycle is proposed / accepted /
    // superseded / deprecated — and `Approved` is the sanctioned "accepted"
    // state (Draft == proposed). Gating it left an ADR with no way out of
    // Draft, while existing ADRs already sat at Approved by convention.
    // trace:BUG-751 | ai:claude
    matches!(
        req_type,
        RequirementType::Vision
            | RequirementType::Epic
            | RequirementType::Principle
            | RequirementType::Constraint
            | RequirementType::Term
    )
}

// BUG-751: type-aware wrapper over `validate_status_input`. The ADR lifecycle
// verb `accepted` is not a status in the enum; for decision-class specs it is
// an input alias for `Approved` (approved == accepted for ADRs), so a recorded
// acceptance can be applied with either verb. For every other type — and every
// other input — this delegates to the canonical validator, except that a
// non-decision given `accepted` gets a refusal naming the correct verb.
// trace:BUG-751 | ai:claude
pub(crate) fn validate_status_input_for_type(
    raw: &str,
    req_type: &RequirementType,
) -> Result<&'static str, String> {
    if raw.trim().eq_ignore_ascii_case("accepted") {
        if *req_type == RequirementType::Decision {
            return Ok("Approved");
        }
        return Err(format!(
            "invalid status `{}` — `accepted` applies to decision specs (where it \
             records as `approved`); expected one of: {}",
            raw, VALID_STATUS_INPUTS
        ));
    }
    validate_status_input(raw)
}

/// TASK-130: resolve the `human_only` marker for a freshly-added spec from its
/// type plus the explicit `--human-only` / `--no-human-only` flags.
///
/// A Spike is, by definition, time-boxed research driven by operator judgment
/// (candidate selection, experimental design, rubric evaluation, report
/// writing) — not heads-down implementation. So a Spike defaults to
/// `human_only = true`, which the orchestrator's pre-pickup gate honors by
/// skipping it rather than handing it to an implementer agent that would have
/// to bail or fabricate work. Every other type defaults to `false`.
///
/// The flags always win, and they are mutually exclusive at the CLI layer
/// (`conflicts_with`), so at most one is set:
/// - `--human-only`     → `true`  (opt any type in)
/// - `--no-human-only`  → `false` (opt a Spike back out into auto-pickup)
/// - neither            → the per-type default above
///
/// trace:TASK-130 | ai:claude
pub(crate) fn resolve_human_only(
    req_type: &RequirementType,
    human_only: bool,
    no_human_only: bool,
) -> bool {
    if human_only {
        return true;
    }
    if no_human_only {
        return false;
    }
    matches!(req_type, RequirementType::Spike)
}

/// TASK-754: why `aida add --queue` would refuse to enqueue the freshly-filed
/// spec. Pure decision over the new spec's *final* status (post-intake-gate)
/// plus whether the intake gate downgraded the requested Approved to Draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueAtFilingRefusal {
    /// The advisor-authority gate downgraded Approved → Draft (AC5): a
    /// non-advisor / non-TTY session filed but cannot queue — same hard gate
    /// `aida queue add` enforces.
    Downgraded,
    /// The spec's status is simply not enqueueable (AC2): only Approved is
    /// groomable, matching `aida backlog groom`'s policy.
    NotApproved,
}

// trace:TASK-754 trace:BUG-631 | ai:claude
/// `None` ⇒ enqueue is allowed. `Some(reason)` ⇒ refuse with the matching
/// message. AC2 + AC5 collapse to one rule: the new spec must end up Approved,
/// and a downgrade (the authority gate having fired) is reported distinctly so
/// the operator knows to re-run as the advisor rather than just fix the status.
///
/// The `route` (the `--for` target, before defaulting) carves out the
/// request-for-review case. Routing a freshly-filed draft `--for advisor`
/// (or `--for human`/`--for reviewer`) is a REQUEST for review/triage, open to
/// any role — so it is never refused here regardless of status, mirroring the
/// standalone `aida queue add --for advisor`, which enqueues any non-terminal
/// status. Only execution-dispatch routes carry the Approved-only / authority
/// gate.
pub(crate) fn queue_at_filing_refusal(
    final_status: &RequirementStatus,
    intake_downgraded: bool,
    route: Option<&str>,
) -> Option<QueueAtFilingRefusal> {
    // BUG-631: a request-for-review route bypasses the dispatch gate entirely.
    if !for_target_requires_dispatch_authority(route) {
        return None;
    }
    if matches!(final_status, RequirementStatus::Approved) {
        return None;
    }
    if intake_downgraded {
        Some(QueueAtFilingRefusal::Downgraded)
    } else {
        Some(QueueAtFilingRefusal::NotApproved)
    }
}

impl OrchestratorBadge {
    pub(crate) fn build(phase: Option<u8>, no_human_mode: Option<&str>) -> Self {
        let phase = match phase {
            Some(n) => match auto_complete::Phase::from_index(n as i32) {
                Some(p) => format!("auto:{n}/6 {}", p.slug()),
                None => format!("auto:{n}/6"),
            },
            None => "auto:?/6".to_string(),
        };
        OrchestratorBadge {
            phase,
            no_human: no_human_mode.map(|m| format!("no-human:{m}")),
            pause: "pause-here",
        }
    }
}

// ============================================================================
// `aida plan verify` — lint a docs/plans/ file against the structured
// template (TASK-92). Reports drifted line refs, missing files, and absent
// required sections; exits non-zero on any hard failure so it can run as a
// pre-commit hook. trace:TASK-93 | ai:claude
// ============================================================================

// `aida skill lint` — the forward-guard for skills that reference an
// implementation plan. A skill under `.claude/skills/` that points at a
// `docs/plans/*.md` file can rot when that plan drifts or is removed; this
// scans each skill for plan references, runs the same checks `aida plan
// verify` runs on each referenced plan (drifted refs / missing files / absent
// sections), and raw-glyph-checks the skill body. Read-only; never rewrites a
// plan or a skill. trace:TASK-927 | ai:claude

// ============================================================================
// TASK-94 — `aida plan helpers <spec>`. Derive a "Reusable helpers" section
// by walking the requirement graph (sibling / same-feature / same-tag specs)
// and harvesting those specs' `// trace:` comments from the codebase, so the
// implementer reuses existing helpers instead of re-inventing them.
// trace:TASK-94 | ai:claude
// ============================================================================

/// TASK-241: extract the PR number from a GitHub squash-merge commit
/// subject, which ends with `(#NN)`. Returns None when the subject has
/// no such suffix. trace:TASK-241 | ai:claude
pub(crate) fn parse_squash_pr_number(subject: &str) -> Option<u64> {
    subject
        .trim_end()
        .rsplit_once("(#")
        .and_then(|(_, tail)| tail.strip_suffix(')'))
        .and_then(|n| n.parse::<u64>().ok())
}

/// TASK-238/STORY-442: the inline tag-chip body for an `aida queue list`
/// row. `batch:*` and `lifecycle:*` tags come first and are always shown
/// (they alter pickup/routing behavior); plain tags are sorted, capped at
/// 3, and any remainder collapses to a `+N` overflow marker. Returns None
/// when the requirement carries no tags. trace:TASK-238 STORY-442
pub(crate) fn format_tag_chip(tags: &std::collections::HashSet<String>) -> Option<String> {
    if tags.is_empty() {
        return None;
    }
    const MAX_PLAIN: usize = 3;
    let mut batch_tags: Vec<&str> = Vec::new();
    let mut lifecycle_tags: Vec<&str> = Vec::new();
    let mut plain_tags: Vec<&str> = Vec::new();
    for t in tags {
        if t.to_ascii_lowercase().starts_with("batch:") {
            batch_tags.push(t.as_str());
        } else if t.to_ascii_lowercase().starts_with("lifecycle:") {
            lifecycle_tags.push(t.as_str());
        } else {
            plain_tags.push(t.as_str());
        }
    }
    batch_tags.sort_unstable();
    lifecycle_tags.sort_unstable();
    plain_tags.sort_unstable();
    let mut shown: Vec<String> = batch_tags.iter().map(|s| s.to_string()).collect();
    shown.extend(lifecycle_tags.iter().map(|s| s.to_string()));
    shown.extend(plain_tags.iter().take(MAX_PLAIN).map(|s| s.to_string()));
    if plain_tags.len() > MAX_PLAIN {
        shown.push(format!("+{}", plain_tags.len() - MAX_PLAIN));
    }
    Some(shown.join(", "))
}

/// TASK-238: does the tag set contain `want` (case-insensitive)? The
/// predicate behind `aida queue list --tag`. trace:TASK-238 | ai:claude
pub(crate) fn tag_matches_exact(tags: &std::collections::HashSet<String>, want: &str) -> bool {
    tags.iter().any(|t| t.eq_ignore_ascii_case(want))
}

/// TASK-238: does any tag start with `prefix` (case-insensitive)? The
/// predicate behind `aida queue list --tag-prefix`. trace:TASK-238
pub(crate) fn tag_matches_prefix(tags: &std::collections::HashSet<String>, prefix: &str) -> bool {
    let lp = prefix.to_ascii_lowercase();
    tags.iter().any(|t| t.to_ascii_lowercase().starts_with(&lp))
}

/// TASK-238: the `batch:*` tag a requirement carries (lowest-sorting
/// when several), used as the group key for `aida queue list
/// --by-batch`. None when the requirement is un-batched.
/// trace:TASK-238 | ai:claude
pub(crate) fn batch_tag_of(tags: &std::collections::HashSet<String>) -> Option<&str> {
    let mut found: Vec<&str> = tags
        .iter()
        .filter(|t| t.to_ascii_lowercase().starts_with("batch:"))
        .map(|s| s.as_str())
        .collect();
    found.sort_unstable();
    found.into_iter().next()
}

/// TASK-270: strip a redundant leading `batch:` prefix off a string,
/// returning `Some(rest)` only when the prefix was present
/// (case-insensitive). `batch:` is the literal tag printed by `aida
/// queue list`; first-users reflexively copy that whole token back into
/// `--batch` or a positional id. trace:TASK-270 | ai:claude
pub(crate) fn strip_batch_prefix(s: &str) -> Option<&str> {
    let prefix = "batch:";
    s.get(..prefix.len())
        .filter(|p| p.eq_ignore_ascii_case(prefix))
        .map(|_| &s[prefix.len()..])
}

/// TASK-270: normalize a `--batch` flag value so `--batch NAME` and the
/// redundant `--batch batch:NAME` (the literal tag from `aida queue
/// list`) both resolve to `NAME`. trace:TASK-270 | ai:claude
pub(crate) fn normalize_batch_name(s: &str) -> &str {
    strip_batch_prefix(s).unwrap_or(s)
}

/// TASK-310: parse comma-separated batch-chain names. Names are kept in the
/// caller's declared order; `batch:` prefixes are tolerated per TASK-270.
pub(crate) fn parse_batch_chain(raw: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for part in raw.split(',') {
        let name = normalize_batch_name(part.trim()).trim();
        if name.is_empty() {
            anyhow::bail!(
                "batch list contains an empty name — use comma-separated names like `--batches A,B,C`"
            );
        }
        names.push(name.to_string());
    }
    if names.is_empty() {
        anyhow::bail!("batch list is empty — use comma-separated names like `--batches A,B,C`");
    }
    Ok(names)
}

/// TASK-270: resolve the effective positional id and batch name for
/// `aida queue work`. Accepts `batch:NAME` as a positional id
/// (equivalent to `--batch NAME`) and strips a redundant `batch:`
/// prefix off the `--batch` flag value. At most one element of the
/// returned `(id, batch)` pair is `Some` — clap's
/// `conflicts_with(batch, id)` already rejects passing both, and a
/// `batch:`-prefixed positional is rerouted to the batch slot.
/// trace:TASK-270 | ai:claude
/// TASK-322: true when a `queue work` positional is a `next` / `nextN`
/// head-pickup keyword (vs a SPEC-ID or `batch:NAME`). Used to catch the
/// `--batch` + `nextN` collision that otherwise silently dropped the keyword.
/// trace:TASK-322 | ai:claude
pub(crate) fn is_next_keyword_id(id: &str) -> bool {
    id.to_ascii_lowercase()
        .strip_prefix("next")
        .map(|suffix| suffix.is_empty() || suffix.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false)
}

pub(crate) fn resolve_queue_work_batch<'a>(
    id: Option<&'a str>,
    batch: Option<&'a str>,
) -> (Option<&'a str>, Option<&'a str>) {
    if let Some(b) = batch {
        return (None, Some(normalize_batch_name(b)));
    }
    if let Some(i) = id {
        if let Some(name) = strip_batch_prefix(i) {
            return (None, Some(name));
        }
        return (Some(i), None);
    }
    (None, None)
}

/// TASK-241: surface git linkage for a spec inside `aida show` — the
/// commits that reference it (AIDA commit format puts `(SPEC-ID)` in
/// the message), the files carrying its `trace:` comments, the branch +
/// worktree the work lives on, and PR state. Every sub-probe is
/// best-effort: a missing `gh`, an absent `origin/main`, or zero
/// matches degrades to a dim note rather than failing the show.
/// `--no-git` skips the whole section; `--verbose` un-caps the commit
/// list and adds per-commit diff stats. trace:TASK-241 | ai:claude
/// Resolve a refname for the default branch that is **known to exist**
/// in `project_root`, so callers can pass it to `git merge-base
/// --is-ancestor` without risking a `fatal: Not a valid object name <X>`
/// stderr splice when the repo's default branch is not `main`.
///
/// Resolution order:
///   1. `origin/HEAD` symbolic ref (the actual default branch on origin)
///   2. `init.defaultBranch` config — `origin/<name>` then bare `<name>`
///   3. The `"main"` / `"master"` literals — `origin/<name>` then bare
///
/// Returns `None` when no candidate ref exists; callers should then
/// skip the ancestor check rather than guess (the prior code passed
/// `"main"` as a literal, which leaked stderr on master-default repos).
/// trace:BUG-380 | ai:claude
pub(crate) fn resolve_default_branch_ref(project_root: &std::path::Path) -> Option<String> {
    use std::process::Command as PCmd;
    let git = |args: &[&str]| -> Option<String> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
            .filter(|s| !s.is_empty())
    };

    if let Some(short) = git(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ]) {
        return Some(short);
    }

    let mut candidates: Vec<String> = Vec::new();
    if let Some(cfg) = git(&["config", "--get", "init.defaultBranch"]) {
        candidates.push(cfg);
    }
    for lit in ["main", "master"] {
        if !candidates.iter().any(|c| c == lit) {
            candidates.push(lit.to_string());
        }
    }

    for name in candidates {
        let origin = format!("origin/{}", name);
        if git(&["rev-parse", "--verify", "--quiet", &origin]).is_some() {
            return Some(origin);
        }
        if git(&["rev-parse", "--verify", "--quiet", &name]).is_some() {
            return Some(name);
        }
    }
    None
}

// ============================================================================
// TASK-113 — `aida ultraplan <SPEC>`. Assemble a rich, structured planning
// prompt from a spec's full context (description, acceptance, related specs,
// the AIDA plan template, trace-graph reusable helpers) so planners anchor on
// concrete requirements instead of a terse user prompt.
// trace:TASK-113 | ai:claude
// ============================================================================

/// The AIDA plan-template section list, inlined into the planning prompt so
/// the returned plan already matches `docs/plans/_TEMPLATE.md` (TASK-92) and
/// `aida plan verify` (TASK-93) passes on the saved plan.
pub(crate) const ULTRAPLAN_STRUCTURE: &str = "\
Produce the plan in AIDA's structured format — a header line with Date / \
Specs / Status / Complexity, then these sections in order:

1. **Approach** — one-paragraph executive summary, plus an ASCII diagram \
where a state machine or control flow would compress prose.
2. **Decisions** — each significant choice resolved, with rationale (not an \
open-questions list).
3. **Files (in build-order)** — symbol-anchored edits per file, ordered so \
each commit builds clean.
4. **Critical Files** — flat list of every must-touch path.
5. **Reusable helpers** — existing helpers to call rather than re-invent \
(seeded below).
6. **Risks + gotchas** — numbered, each with a mitigation.
7. **Tests** — named test functions, not \"add tests\".
8. **Verification** — an executable bash smoke test (positive + negative).
9. **Followups** — out-of-scope items to file as TASKs at completion time.
10. **Related** — builds-on / blocks / see-also links.";

/// Does the project's working tree have uncommitted changes? Conservative: any
/// `git status --porcelain` output (excluding the gitignored store/cache noise
/// git already omits) counts as dirty. trace:STORY-659
pub(crate) fn working_tree_is_dirty(project_root: &std::path::Path) -> bool {
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(project_root)
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

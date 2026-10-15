//! CI probing: CiProbe, decide_ci_action and wait_for_ci_terminal (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

pub(crate) fn branch_behind_main(
    repo: &std::path::Path,
    branch: &str,
) -> Option<(u64, Vec<String>)> {
    if branch == "main" || branch == "HEAD" {
        return None;
    }
    let rev_parse = |refname: &str| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                git_arg_guard::END_OF_OPTIONS,
                refname,
            ]) // trace:BUG-1622 | ai:claude
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    // Prefer origin/main when it exists — local main can be days behind.
    let main_ref = rev_parse("origin/main")
        .map(|_| "origin/main")
        .or_else(|| rev_parse("main").map(|_| "main"))?;
    let range = format!("{}..{}", branch, main_ref);
    let count_out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        // trace:BUG-1622 | ai:claude
        .args(["rev-list", "--count", git_arg_guard::END_OF_OPTIONS, &range])
        .output()
        .ok()?;
    if !count_out.status.success() {
        return None;
    }
    let count: u64 = String::from_utf8_lossy(&count_out.stdout)
        .trim()
        .parse()
        .ok()?;
    if count == 0 {
        return None;
    }
    let log_out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", &range, "--pretty=format:%h %s", "--no-merges"])
        .output()
        .ok()?;
    let sample: Vec<String> = String::from_utf8_lossy(&log_out.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    Some((count, sample))
}

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

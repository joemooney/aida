//! `aida drain` command cluster — `handle_drain_command`, `drain_clear`,
//! and the drain-resume handlers (`handle_drain_resume` + resume probes),
//! extracted from `lib.rs` (SPIKE-78 / STORY-771; pure movement, no
//! behavior change). The no-human gate and `handle_from_pr` stay in
//! `lib.rs`.
// trace:STORY-771 | ai:claude

use crate::*;
use anyhow::Context;
use serde::{Deserialize, Serialize};

const DRAIN_STOP_FILE: &str = "drain-stop.json";
const DRAIN_STOP_ENV: &str = "AIDA_DRAIN_STOP_FILE";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DrainStopRequest {
    requested_at_utc: String,
    mode: String,
    pid: Option<u32>,
}

/// `aida drain status` — show the active `aida queue work --auto-complete`
/// drain (STORY-301). Reads `.aida/drain-state.json`, corroborates the recorded
/// orchestrator PID against a liveness probe, and prints the human summary —
/// or `No drain in progress.` (exit 0) when no drain is running. `--clear`
/// removes a stale file left by a crashed orchestrator. trace:STORY-301
// BUG-759: launcher-held drains (`aida burndown run`, `aida queue integrate`)
// hold `.aida/drain.lock` for their entire wall-clock but write no drain-state
// file, so the state-file read alone said "No drain in progress" mid-burndown.
// When the state file is absent (or a stale tombstone) and the lock's pid is
// LIVE, report the drain from the lock instead. trace:BUG-759 | ai:claude
pub(crate) fn handle_drain_command(cmd: &DrainCommand) -> Result<()> {
    match cmd {
        DrainCommand::Start {
            selector,
            project,
            batch,
            once,
            no_human,
            role,
            max_failures,
            max_iterations,
            max_runtime,
            max_tokens,
            json,
            vendor,
            model,
        } => handle_drain_start(
            selector.as_deref(),
            project.as_deref(),
            batch.as_deref(),
            *once,
            no_human.as_deref(),
            role.as_deref(),
            *max_failures,
            *max_iterations,
            max_runtime.as_deref(),
            *max_tokens,
            *json,
            vendor.as_deref(),
            model.as_deref(),
        ),
        DrainCommand::Resume { spec, json } => handle_shelved_resume(spec, *json),
        DrainCommand::Stop { now, project } => handle_drain_stop(project.as_deref(), *now),
        DrainCommand::Clear => {
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let status = drain_state::probe(&project_root);
            let json_output = output_format_is_json();
            drain_clear(&project_root, &status, json_output)
        }
        DrainCommand::Status { json, clear } => {
            // Resolve the shared `.aida/` root from any worktree so a child in
            // a sibling worktree reads the *orchestrator's* drain-state file.
            // On a resolution failure the lookup simply finds nothing — the
            // verdict fails safe to "no drain".
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let status = drain_state::probe(&project_root);
            let json_output = *json || output_format_is_json();
            if *clear {
                note_hidden_alias(concat!("aida drain status ", "--clear"), "aida drain clear");
                return drain_clear(&project_root, &status, json_output);
            }
            // BUG-759: corroborate the launcher-held drain lock alongside the
            // orchestrator drain-state file. A live lock is authoritative when
            // the state file can't speak for the drain (absent, or a dead-pid
            // tombstone a killed orchestrator left behind).
            let live_lock = match drain_lock::probe_lock(&project_root) {
                drain_lock::LockStatus::Running(lock) => Some(lock),
                _ => None,
            };
            if let (
                drain_state::DrainStatus::None | drain_state::DrainStatus::Stale(_),
                Some(lock),
            ) = (&status, &live_lock)
            {
                let stale_state = matches!(status, drain_state::DrainStatus::Stale(_));
                if json_output {
                    println!("{}", drain_state::render_lock_json(lock, stale_state));
                } else if agent_output_mode() {
                    println!("{}", drain_state::render_lock_toon(lock, stale_state));
                } else {
                    print!("{}", drain_state::render_lock_human(lock, stale_state));
                    // TASK-294 parity with the state-backed report: surface any
                    // pending worker directives alongside the drain summary.
                    let directives =
                        worker::parse_directives(&worker::worker_cmd_path(&project_root));
                    if let Some(line) = worker::status_line(&directives) {
                        println!();
                        println!("  {line}");
                    }
                }
                return Ok(());
            }
            if json_output {
                println!(
                    "{}",
                    drain_state::render_json_with_context(&status, &project_root)
                );
                return Ok(());
            }
            if agent_output_mode() {
                println!(
                    "{}",
                    drain_state::render_toon_with_context(&status, &project_root)
                );
                return Ok(());
            }
            match status {
                drain_state::DrainStatus::None => {
                    println!("No drain in progress.");
                }
                drain_state::DrainStatus::Active(state) => {
                    print!(
                        "{}",
                        drain_state::render_human_with_context(&state, false, &project_root)
                    );
                }
                drain_state::DrainStatus::Stale(state) => {
                    print!(
                        "{}",
                        drain_state::render_human_with_context(&state, true, &project_root)
                    );
                }
            }
            // TASK-294: equal visibility for the worker control channel —
            // the directive FIFO surfaces here alongside the drain summary
            // so a user sees both at once. Silent when no directives are
            // pending so quiet projects stay quiet.
            let directives = worker::parse_directives(&worker::worker_cmd_path(&project_root));
            if let Some(line) = worker::status_line(&directives) {
                println!();
                println!("  {line}");
            }
            Ok(())
        }
        // `aida drain tail` is the discoverable drain-cluster spelling for
        // the existing `aida tail drain` stream.
        // trace:TASK-1209 | ai:codex
        DrainCommand::Tail {
            json,
            lines,
            since,
            no_follow,
            with_tools,
            no_timestamp,
            annotate,
        } => handle_tail_cli(
            Some("drain".to_string()),
            false,
            *json,
            *lines,
            since.as_deref(),
            *no_follow,
            *with_tools,
            *no_timestamp,
            *annotate,
        ),
    }
}

/// `aida drain resume <SPEC>` — recover the latest deliberately parked run.
/// The event stream is authoritative for both the failed phase and the PR
/// produced by that same run; using the correlated `PhaseDonePr` prevents a
/// stale or unrelated open PR from being reviewed or merged.
// trace:TASK-1272 | ai:codex
fn handle_shelved_resume(spec: &str, json: bool) -> Result<()> {
    let project_root = find_main_worktree_root().or_else(|_| std::env::current_dir())?;
    let events = events::read_all(&project_root);
    let shelf = events::latest_spec_shelved(&events, spec).ok_or_else(|| {
        anyhow::anyhow!(
            "no recorded failure for `{spec}` — `aida drain resume` only re-drives a shelved run"
        )
    })?;
    let storage = Storage::new(project_root.join(".aida-store"));
    if !requirement_has_failure_reason(&storage, spec) {
        anyhow::bail!(
            "`{spec}` is no longer shelved; its last shelve was at {} but it has since recovered",
            shelf.ts
        );
    }
    let phase_slug = match &shelf.kind {
        events::EventKind::SpecShelved { phase, .. } => phase.as_str(),
        _ => unreachable!("latest_spec_shelved returned a non-shelving event"),
    };
    let phase = drain_resume::shelved_resume_phase(phase_slug).ok_or_else(|| {
        anyhow::anyhow!(
            "recorded failure for `{spec}` is at `{phase_slug}`; resume requires an existing PR and supports phases ci through build"
        )
    })?;
    let pr = events
        .iter()
        .rev()
        .find(|event| {
            event.run_uuid == shelf.run_uuid
                && event
                    .spec
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case(spec))
                && matches!(event.kind, events::EventKind::PhaseDonePr { .. })
        })
        .and_then(|event| match event.kind {
            events::EventKind::PhaseDonePr { pr } => Some(pr),
            _ => None,
        })
        .ok_or_else(|| {
            anyhow::anyhow!("no PhaseDonePr was recorded for `{spec}` in the shelved run")
        })?;

    eprintln!(
        "{} resuming `{}` at phase {} ({}) with PR-{} from run {}",
        "↩".cyan().bold(),
        spec,
        phase.index(),
        phase.slug(),
        pr,
        shelf.run_uuid
    );
    let exe = aida_exe_path();
    let mut cmd = std::process::Command::new(exe);
    cmd.current_dir(&project_root)
        .args([
            "queue",
            "work",
            &format!("PR-{pr}"),
            "--auto-complete",
            "--from-pr",
            "--force-claim",
        ])
        .env("AIDA_DRAIN_RESUME_PHASE", phase.slug());
    if json {
        cmd.arg("--json");
    }
    let status = cmd.status().context("launching the resumed drain")?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "resumed drain exited with status {}",
            status.code().unwrap_or(-1)
        )
    }
}

// trace:STORY-1130 | ai:codex
#[allow(clippy::too_many_arguments)]
fn handle_drain_start(
    selector: Option<&str>,
    project: Option<&str>,
    batch: Option<&str>,
    once: bool,
    no_human: Option<&str>,
    role: Option<&str>,
    max_failures: Option<usize>,
    max_iterations: Option<u64>,
    max_runtime: Option<&str>,
    max_tokens: Option<u64>,
    json: bool,
    vendor: Option<&str>,
    model: Option<&str>,
) -> Result<()> {
    let root = autoprogress::resolve_project(project)?;
    if let drain_lock::LockStatus::Running(lock) = drain_lock::probe_lock(&root) {
        anyhow::bail!(
            "a drain is already running (pid {}, started {}, cmd `{}`)",
            lock.pid,
            lock.started_at_utc,
            lock.command
        );
    }

    clear_stop_request(&root);
    let mut args: Vec<String> = vec!["queue".into(), "work".into()];
    if let Some(sel) = selector.filter(|s| !s.trim().is_empty()) {
        args.push(sel.to_string());
    } else if once {
        args.push("next".into());
    }
    args.push("--auto-complete".into());
    if selector.is_none() && batch.is_none() && !once {
        args.push("--drain".into());
    }
    if let Some(batch) = batch.filter(|s| !s.trim().is_empty()) {
        args.push("--batch".into());
        args.push(batch.to_string());
    }
    args.push(format!("--no-human={}", no_human.unwrap_or("both")));
    if let Some(role) = role.filter(|s| !s.trim().is_empty()) {
        args.push("--role".into());
        args.push(role.to_string());
    }
    if let Some(n) = max_failures {
        args.push("--max-failures".into());
        args.push(n.to_string());
    }
    if let Some(n) = max_iterations {
        args.push("--max-iterations".into());
        args.push(n.to_string());
    }
    if let Some(v) = max_runtime.filter(|s| !s.trim().is_empty()) {
        args.push("--max-runtime".into());
        args.push(v.to_string());
    }
    if let Some(n) = max_tokens {
        args.push("--max-tokens".into());
        args.push(n.to_string());
    }
    if json {
        args.push("--json".into());
    }
    if let Some(vendor) = vendor.filter(|s| !s.trim().is_empty()) {
        args.push("--vendor".into());
        args.push(vendor.to_string());
    }
    if let Some(model) = model.filter(|s| !s.trim().is_empty()) {
        args.push("--model".into());
        args.push(model.to_string());
    }

    if !json {
        eprintln!("aida drain start -> aida {}", args.join(" "));
        eprintln!("  project: {}", root.display().to_string().cyan());
        eprintln!("  stop:    aida drain stop");
    }

    let exe = crate::aida_exe_path();
    let status = std::process::Command::new(exe)
        .current_dir(&root)
        .args(&args)
        .env(DRAIN_STOP_ENV, drain_stop_path(&root))
        .status()
        .with_context(|| format!("failed to run `aida {}`", args.join(" ")))?;
    std::process::exit(status.code().unwrap_or(1));
}

// trace:STORY-1130 | ai:codex
fn handle_drain_stop(project: Option<&str>, now: bool) -> Result<()> {
    let root = autoprogress::resolve_project(project)?;
    let lock = match drain_lock::probe_lock(&root) {
        drain_lock::LockStatus::Running(lock) => Some(lock),
        _ => None,
    };
    if lock.is_none() {
        println!("No drain in progress.");
        return Ok(());
    }
    let lock = lock.unwrap();
    write_stop_request(&root, if now { "now" } else { "graceful" }, Some(lock.pid))?;
    if now {
        let _ = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(lock.pid.to_string())
            .status();
        let _ = std::fs::remove_file(drain_lock::drain_lock_path(&root));
        let _ = drain_state::DrainState::clear(&root);
        println!("Hard stop requested for drain pid {}.", lock.pid);
    } else {
        println!(
            "Graceful stop requested for drain pid {}. The in-flight phase may finish; no new spec will be picked up.",
            lock.pid
        );
    }
    Ok(())
}

pub(crate) fn drain_stop_path(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".aida").join(DRAIN_STOP_FILE)
}

pub(crate) fn install_stop_request_env(project_root: &std::path::Path) {
    std::env::set_var(DRAIN_STOP_ENV, drain_stop_path(project_root));
}

pub(crate) fn clear_stop_request(project_root: &std::path::Path) {
    let _ = std::fs::remove_file(drain_stop_path(project_root));
}

pub(crate) fn stop_requested_from_env() -> bool {
    std::env::var_os(DRAIN_STOP_ENV)
        .map(std::path::PathBuf::from)
        .map(|p| p.exists())
        .unwrap_or(false)
}

fn write_stop_request(project_root: &std::path::Path, mode: &str, pid: Option<u32>) -> Result<()> {
    let path = drain_stop_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let req = DrainStopRequest {
        requested_at_utc: chrono::Utc::now().to_rfc3339(),
        mode: mode.to_string(),
        pid,
    };
    let json = serde_json::to_string_pretty(&req)?;
    aida_core::write_atomic(&path, json)?;
    Ok(())
}

/// `aida drain clear` — remove a stale drain-state file. Refuses
/// while the orchestrator is still live: a live orchestrator removes the file
/// itself on a clean exit, so clearing it from under a running drain would
/// only hide work in progress. trace:STORY-301 | ai:claude
pub(crate) fn drain_clear(
    project_root: &std::path::Path,
    status: &drain_state::DrainStatus,
    json: bool,
) -> Result<()> {
    match status {
        drain_state::DrainStatus::None => {
            if json {
                println!("{{\"status\":\"none\"}}");
            } else {
                println!("No drain-state file to clear.");
            }
            Ok(())
        }
        drain_state::DrainStatus::Active(state) => anyhow::bail!(
            "the drain is still live (orchestrator pid {}) — not clearing. \
             The drain-state file is removed automatically when the \
             orchestrator exits.",
            state.orchestrator_pid
        ),
        drain_state::DrainStatus::Stale(_) => {
            drain_state::DrainState::clear(project_root)?;
            if json {
                println!("{{\"status\":\"cleared\"}}");
            } else {
                println!(
                    "{} removed the stale drain-state file",
                    crate::glyph(crate::glyphs::Glyph::Check).green().bold()
                );
            }
            Ok(())
        }
    }
}

/// STORY-492: clamp a reconciled resume phase to one a *fresh* resume process
/// can actually run. Phases 1-2 are coupled to the implementer session/lease,
/// which a restarted process does not hold: phase 1 (branch absent) re-runs the
/// whole drain cleanly, but a reconciled CI(2) — implementer done, CI
/// postcondition unmet — is bumped to the reviewer (phase 3), because the
/// lease-coupled CI-end step cannot be replayed and the reviewer + merge phases
/// re-establish gating (`gh pr merge` still respects required checks). Pure so
/// the safety clamp is unit-pinned. trace:STORY-492 | ai:claude
pub(crate) fn clamp_resume_start_phase(reconciled: auto_complete::Phase) -> auto_complete::Phase {
    if reconciled == auto_complete::Phase::Ci {
        auto_complete::Phase::Reviewer
    } else {
        reconciled
    }
}

/// BUG-478: does the requirement for `spec` carry a `failure_reason` — the TRUE
/// "deliberately shelved" signal? The shelve function (`finish_failure`) sets
/// `req.failure_reason` (+ usually `NeedsAttention`) on a shelvable phase
/// failure; the drain-state member's `STATE_FAILED` is stamped on ANY non-zero
/// outcome and so cannot distinguish a deliberate shelve from a non-shelve
/// resume failure. Resolves spec→requirement the same way `probe_resume_facts`
/// resolves `spec_completed` (matching `spec_id` OR `agreed_id`) so agreed/raw
/// ids behave identically. Returns `false` when the store can't be loaded or the
/// requirement isn't found — a missing req must not wedge resume into the
/// LeaveShelved branch. trace:BUG-478 | ai:claude
pub(crate) fn requirement_has_failure_reason(storage: &Storage, spec: &str) -> bool {
    storage
        .load()
        .ok()
        .and_then(|store| {
            store
                .requirements
                .iter()
                .find(|r| {
                    r.spec_id.as_deref() == Some(spec) || r.agreed_id.as_deref() == Some(spec)
                })
                .map(|r| r.failure_reason.is_some())
        })
        .unwrap_or(false)
}

/// STORY-492 (slice 2c): probe the real world for a crashed drain member's
/// per-phase postconditions, returning the [`drain_resume::ResumeFacts`] plus
/// the branch + PR the re-entry must seed into the driver.
///
/// Conservative by design: a postcondition we cannot confirm stays `false`
/// (re-run that phase). Re-running an already-merged `merge()` is redeemed by
/// the BUG-241 reconcile, and CI / reviewer / build are idempotent — so the
/// only postconditions that MUST be accurate are the ones gating the
/// irreversible tail: `pr_merged` (don't re-merge) and `spec_completed` (don't
/// re-pull). trace:STORY-492 | ai:claude
pub(crate) fn probe_resume_facts(
    project_root: &std::path::Path,
    storage: &Storage,
    spec: &str,
    member: Option<&drain_state::DrainMember>,
) -> (drain_resume::ResumeFacts, Option<String>, Option<u32>) {
    let mut sink = network_retry::StderrSink;

    // PR number: prefer what drain-state recorded; else resolve the same
    // forge-first review surface that `aida review` uses. A PR-only
    // `queue work --from-pr` has no lease/drain member, so the open PR must be
    // discoverable by spec trailer or spec-named head branch alone.
    // trace:BUG-881 | ai:codex
    let mut pr = member.and_then(|m| m.pr);
    let mut resolved_branch: Option<String> = None;
    if pr.is_none() {
        let ids = vec![spec.to_string()];
        let linkage = collect_git_linkage_opts(project_root, &ids, false);
        if let ReviewSurface::OpenChange { branch, number, .. } =
            resolve_review_surface_forge_first(project_root, spec, &linkage)
        {
            pr = Some(number as u32);
            resolved_branch = Some(branch);
        }
    }
    // The PR's head branch (the merge / CI phases probe against it).
    let branch =
        resolved_branch.or_else(|| pr.and_then(|n| pr_head_branch(project_root, n as u64)));

    // pr_merged — ACCURATE (gates skipping the irreversible merge).
    let pr_merged = pr
        .and_then(|n| pr_is_merged_with_sink(project_root, n, &mut sink))
        .unwrap_or(false);

    // branch_exists — a PR (open or merged) exists, or the branch is on origin.
    let branch_exists = pr.is_some()
        || branch
            .as_deref()
            .map(|b| {
                matches!(
                    probe_branch_on_origin(project_root, b),
                    BranchOriginProbe::Present
                )
            })
            .unwrap_or(false);

    // ci_green — a point-in-time CI probe; a merged PR implies CI cleared.
    // STORY-516: forge-routed. The branch-origin probe above and the verdict
    // path below both resolve against `project_root`; this probe did not, so a
    // resume driven from outside the worktree asked the wrong repository's
    // forge. trace:TASK-1273 | ai:claude
    let ci_forge_kind = crate::forge::resolve_forge_kind(project_root);
    let ci_green = pr_merged
        || branch
            .as_deref()
            .map(|b| {
                matches!(
                    ci_probe_with_forge(project_root, ci_forge_kind, b),
                    CiProbe::Green { .. }
                )
            })
            .unwrap_or(false);

    // reviewed — an Approved verdict file exists for the PR.
    // The spelling is insufficient: prove it names the current branch tip.
    // trace:BUG-1466 | ai:codex
    // trace:BUG-1538 | ai:codex
    let current_head = pr.and_then(|n| {
        let change = crate::forge::ChangeRef {
            id: n as u64,
            url: String::new(),
            branch: branch.clone().unwrap_or_default(),
            base: String::new(),
            title: None,
        };
        crate::forge::forge_for(project_root)
            .change_status(&change)
            .ok()
            .map(|status| status.head_sha)
            .filter(|sha| !sha.trim().is_empty())
    });
    let reviewed = pr
        .map(|n| {
            let Some(head) = current_head.as_deref() else {
                return false;
            };
            let paths = [
                review_verdict::verdict_path(project_root, &format!("PR-{n}")),
                review_verdict::verdict_path(project_root, spec),
            ];
            let bodies: Vec<String> = match paths
                .iter()
                .filter(|path| path.exists())
                .map(std::fs::read_to_string)
                .collect::<Result<Vec<String>, _>>()
            {
                Ok(bodies) if !bodies.is_empty() => bodies,
                _ => return false,
            };
            matches!(
                review_verdict::reconcile_artifacts_for_sha(
                    bodies.iter().map(String::as_str),
                    head
                ),
                Ok(Some(review_verdict::VerdictKind::Approved))
            )
        })
        .unwrap_or(false);

    // spec_completed — ACCURATE (gates skipping the pull/auto-bump).
    let spec_completed = storage
        .load()
        .ok()
        .and_then(|store| {
            store
                .requirements
                .iter()
                .find(|r| {
                    r.spec_id.as_deref() == Some(spec) || r.agreed_id.as_deref() == Some(spec)
                })
                .map(|r| r.status == RequirementStatus::Completed)
        })
        .unwrap_or(false);

    let facts = drain_resume::ResumeFacts {
        branch_exists,
        ci_green,
        reviewed,
        pr_merged,
        // Build is idempotent — always re-run on resume (plan note).
        build_ok: false,
        spec_completed,
    };
    (facts, branch, pr)
}

/// STORY-492 (slice 2d): the `--resume-drain` entry point. Reads the
/// crashed-drain state, runs the PID-liveness gate (refuse if the original
/// orchestrator is still alive — the catastrophic double-drive guard),
/// reconciles the re-entry phase from probed git/PR/spec reality, prints the
/// decision, and — unless `--dry-run` — re-enters the current member at the
/// reconciled phase. Never returns. trace:STORY-492 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_drain_resume(
    storage: &Storage,
    user_id: &str,
    drain_id: Option<&str>,
    dry_run: bool,
    json: bool,
    permission_mode: Option<&str>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    steal: bool,
    force_claim: bool,
    allow_stale_base: bool,
    no_auto_rebase: bool,
) -> ! {
    let project_root = match storage.path().parent() {
        Some(p) => p.to_path_buf(),
        None => {
            eprintln!(
                "{} cannot derive project root from the store path",
                crate::glyph(crate::glyphs::Glyph::Cross)
            );
            std::process::exit(1);
        }
    };
    let Some(state) = drain_state::DrainState::read(&project_root) else {
        eprintln!(
            "{} no `.aida/drain-state.json` — there is no crashed drain to resume.",
            crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
        );
        eprintln!(
            "  {} a clean drain removes its state file on exit; only a crashed/killed \
             drain leaves one behind.",
            "→".dimmed()
        );
        std::process::exit(1);
    };

    // Optional `--drain-id` corroboration — match the run UUID or the start
    // timestamp so a stale state file isn't resumed by mistake.
    if let Some(id) = drain_id {
        let matches = state.run_uuid == id || state.started_at.starts_with(id);
        if !matches {
            eprintln!(
                "{} --drain-id `{}` does not match the recorded drain (run `{}`, started {}).",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                id,
                state.run_uuid,
                state.started_at
            );
            std::process::exit(1);
        }
    }

    // PID-liveness gate — the catastrophic double-drive guard. Conservative:
    // any doubt resolves to "alive". trace:STORY-492 | ai:claude
    let orchestrator_alive = process_probe::pid_is_alive(state.orchestrator_pid);

    let current = state.current.clone();
    let member = current
        .as_ref()
        .and_then(|c| state.members.iter().find(|m| &m.spec == c))
        .cloned();
    let member_in_flight = current.is_some();
    let member_state_in_phase = member.as_ref().map(|m| m.is_running()).unwrap_or(false);
    // BUG-478: the "deliberately shelved" signal lives on the REQUIREMENT
    // (`failure_reason`, set by the shelve function), NOT on the drain-state
    // member. The member is stamped STATE_FAILED on ANY non-zero outcome —
    // including a non-shelve resume failure (spawn / internal error) — so
    // keying `has_failure_reason` off `member.state == STATE_FAILED` conflated
    // "failed resume" with "deliberately shelved", wedging a re-resumable crash
    // into the LeaveShelved branch. Read the requirement's `failure_reason`
    // instead: a genuinely shelved spec still has it set (→ Shelved), but a
    // non-shelve resume failure leaves it None (→ ResumableCrash, restoring
    // BUG-438's keep-state-and-re-resume intent). A requirement we can't load
    // defaults to not-shelved (resumable) — a missing req must not wedge resume.
    // The spec→requirement resolution mirrors `probe_resume_facts`'s
    // spec_completed lookup so agreed/raw ids match identically.
    // trace:BUG-478 | ai:claude
    let has_failure_reason = current
        .as_deref()
        .map(|spec| requirement_has_failure_reason(storage, spec))
        .unwrap_or(false);

    // Probe the world for the current member's per-phase postconditions.
    let (facts, branch, pr) = match &current {
        Some(spec) => probe_resume_facts(&project_root, storage, spec, member.as_ref()),
        None => (drain_resume::ResumeFacts::default(), None, None),
    };

    let outcome = drain_resume::resume_plan(
        orchestrator_alive,
        member_in_flight,
        member_state_in_phase,
        has_failure_reason,
        &facts,
    );

    let spec_label = current.as_deref().unwrap_or("<none>");
    match outcome {
        drain_resume::ResumeOutcome::RefuseOrchestratorAlive => {
            eprintln!(
                "{} refusing to resume — the original orchestrator (pid {}) is still alive.",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                state.orchestrator_pid
            );
            eprintln!(
                "  {} resuming a live drain would DOUBLE-DRIVE the same spec (two processes \
                 merging the same PR). Stop the running drain first, then re-run --resume-drain.",
                "→".dimmed()
            );
            std::process::exit(1);
        }
        drain_resume::ResumeOutcome::NothingToResume => {
            println!(
                "{} nothing to resume — no member was mid-flight in `{}`.",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                drain_state::drain_state_path(&project_root).display()
            );
            std::process::exit(0);
        }
        drain_resume::ResumeOutcome::LeaveShelved => {
            println!(
                "{} `{}` was deliberately shelved, not crashed — leave it parked and triage \
                 with `aida findings list` (don't resume).",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                spec_label
            );
            std::process::exit(0);
        }
        drain_resume::ResumeOutcome::AlreadyComplete => {
            println!(
                "{} every phase of `{}` is already complete — clearing the stale drain-state.",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                spec_label
            );
            let _ = drain_state::DrainState::clear(&project_root);
            std::process::exit(0);
        }
        drain_resume::ResumeOutcome::ResumeAt(reconciled) => {
            // SAFETY clamp: phases 1-2 (implementer + CI-wait) are coupled to the
            // implementer session/lease, which a fresh resume process does not
            // hold. Phase 1 (branch absent) re-runs the whole drain cleanly. A
            // reconciled CI(2) means the implementer finished but CI's
            // postcondition is unmet — we cannot re-run the lease-coupled CI-end
            // step, so re-enter at the reviewer (phase 3); the reviewer + merge
            // re-establish gating and `gh pr merge` still respects required
            // checks. trace:STORY-492 | ai:claude
            let start_phase = clamp_resume_start_phase(reconciled);
            if reconciled != start_phase {
                println!(
                    "{} reconciled to phase {} (CI) — re-entering at phase {} (reviewer) instead \
                     (CI-wait is coupled to the implementer session a resume cannot hold).",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    reconciled.index(),
                    start_phase.index()
                );
            }
            println!(
                "{} resuming `{}` at phase {} ({}) — earlier phases reconciled as already done.",
                "↩".cyan().bold(),
                spec_label,
                start_phase.index(),
                start_phase.slug()
            );
            if let Some(n) = pr {
                println!("  {} seeded PR-{}", "→".dimmed(), n);
            }
            if dry_run {
                println!(
                    "  {} --dry-run — not re-entering. Drop --dry-run to resume.",
                    "→".dimmed()
                );
                std::process::exit(0);
            }
            // Live re-entry: phase 1 means a from-scratch redo (no seed); any
            // later phase seeds the probed branch + PR.
            let spec = current.expect("ResumeAt implies a current member");
            let variant = auto_complete::AutoCompleteVariant::Full;
            let resume_entry = if start_phase == auto_complete::Phase::Implementer {
                None
            } else {
                Some(ResumeEntry {
                    start_phase,
                    branch,
                    pr,
                    from_pr: false,
                })
            };
            // BUG-438: when we re-enter past phase 1, the crashed implementer's
            // lease on this spec may still be held (a fast resume outruns the
            // mtime-based auto-release). Release it now — process-dead +
            // clean-worktree only — so the reviewer phase doesn't collide with
            // it (it resolves PR→spec and would otherwise hit "scope owned by
            // lease …"). trace:BUG-438 | ai:claude
            if resume_entry.is_some() {
                release_dead_leases_for_resume(&project_root, &spec);
            }
            let result = run_auto_complete(
                storage,
                user_id,
                &spec,
                variant,
                json,
                permission_mode,
                no_human,
                escalate_mode,
                // The resume owns the drain-state file (updates it, clears on a
                // clean finish).
                true,
                steal,
                force_claim,
                allow_stale_base,
                no_auto_rebase,
                resume_entry,
            );
            // TASK-1054: collapse the failed-phase index to the canonical
            // 0/2/3 process code so a wrapping script can branch on the outcome.
            std::process::exit(result.process_exit_code());
        }
    }
}

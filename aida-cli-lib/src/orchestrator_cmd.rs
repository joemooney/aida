//! `aida orchestrator` command cluster — `handle_orchestrator_command` and
//! the co-located state-snapshot handler, extracted from `lib.rs`
//! (SPIKE-78 / STORY-771; pure movement, no behavior change).
// trace:STORY-771 | ai:claude

use crate::*;

/// `aida orchestrator status` — print the corroborated orchestrator context
/// of the current process. The bare status word goes to stdout (so a skill can
/// branch on it cleanly); any informational note goes to stderr.
/// trace:BUG-233 | ai:claude
pub(crate) fn handle_orchestrator_command(cmd: &OrchestratorCommand) -> Result<()> {
    match cmd {
        OrchestratorCommand::Status { json } => {
            // Resolve the shared `.aida/` root from any worktree so a child in
            // a sibling worktree reads the *orchestrator's* marker dir. On a
            // resolution failure the marker lookup simply finds nothing — the
            // verdict fails safe to interactive. trace:BUG-233 | ai:claude
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let ctx = orchestrator::detect(&project_root);
            if *json {
                println!(
                    "{{\"context\":\"{}\",\"corroborated\":{},\"reason\":\"{}\"}}",
                    ctx.status_word(),
                    ctx.is_orchestrated(),
                    ctx.reason_slug()
                );
            } else {
                println!("{}", ctx.status_word());
            }
            // The note is informational, never alarming — BUG-233's corrected
            // diagnosis: a bare `AIDA_AUTO_COMPLETE` is not a leak to chase.
            if let Some(note) = ctx.informational_note() {
                eprintln!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    note.dimmed()
                );
            }
            Ok(())
        }
    }
}

/// `aida state-snapshot --spec <SPEC-ID>` — emit the seven-row
/// finish-state preamble deterministically (TASK-391). Reads the spec from
/// the git-canonical store, the branch + ahead/upstream facts from `git`,
/// the open PR from `gh pr list`, the drain phase + mode from
/// `.aida/drain-state.json`, the orchestrator on/off from the same
/// corroboration `aida orchestrator status` uses, and the plan path from
/// the active session's manifest. Tests / fmt are caller-supplied (the
/// skill knows what it just ran). trace:TASK-391 | ai:claude
pub(crate) fn handle_state_snapshot_command(
    backend: &dyn aida_core::DatabaseBackend,
    store_path: &std::path::Path,
    spec: &str,
    tests: &str,
    fmt: &str,
    json: bool,
) -> Result<()> {
    let snap = gather_state_snapshot(backend, store_path, spec, tests, fmt)?;
    let out = if json {
        snap.render_json()
    } else {
        snap.render_text()
    };
    println!("{}", out);
    Ok(())
}

/// Assemble the [`state_snapshot::StateSnapshot`] from the live process
/// context — spec store + git + gh + drain-state + manifest. Kept as a
/// gathering function (no rendering) so the renderer stays unit-testable
/// with hand-constructed snapshots.
pub(crate) fn gather_state_snapshot(
    backend: &dyn aida_core::DatabaseBackend,
    store_path: &std::path::Path,
    spec_id: &str,
    tests: &str,
    fmt: &str,
) -> Result<state_snapshot::StateSnapshot> {
    use state_snapshot::{BranchRow, PlanRow, PrRow, SpecRow, StateSnapshot};

    let req = backend
        .get_requirement_by_spec_id(spec_id)?
        .ok_or_else(|| anyhow::anyhow!("requirement not found: {spec_id}"))?;
    let spec = SpecRow {
        id: req.display_id(),
        title: req.title.clone(),
        status: req.status.to_string(),
    };

    // `project_root` is the current (possibly session) worktree — it holds
    // this session's branch, its PR, and its `.aida/sessions/` symlink. The
    // *main* worktree root holds the shared drain-state and orchestrator
    // marker dirs that every worktree of the same project sees. Resolve
    // both: branch/PR/manifest read from `project_root`; drain + orchestrator
    // read from `main_root`. trace:TASK-391 | ai:claude
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let main_root = find_main_worktree_root().unwrap_or_else(|_| project_root.clone());

    let branch = current_branch_at(&project_root).map(|name| {
        let ahead_main = ahead_behind_vs_ref(&project_root, &name, "origin/main")
            .or_else(|| ahead_behind_vs_ref(&project_root, &name, "main"))
            .map(|(a, _)| a);
        let push_status = describe_push_status(&project_root, &name);
        BranchRow {
            name,
            ahead_main,
            push_status,
        }
    });

    let pr = match branch.as_ref() {
        // STORY-516: forge-routed. trace:STORY-516 | ai:claude
        Some(b) => match change_lookup_for_branch(&project_root, &b.name) {
            crate::forge::ChangeLookup::Found(c) => PrRow::Open {
                number: c.id,
                url: c.url,
            },
            crate::forge::ChangeLookup::NoChange => PrRow::None,
            crate::forge::ChangeLookup::CliMissing => PrRow::GhMissing,
            crate::forge::ChangeLookup::Unreachable(detail) => PrRow::GhUnreachable { detail },
            crate::forge::ChangeLookup::CliFailed(detail) => PrRow::GhFailed { detail },
        },
        None => PrRow::None,
    };

    let drain = describe_drain_row(&main_root);

    let plan = locate_plan_for_active_session(&project_root)
        .map(|path| PlanRow::File { path })
        .unwrap_or(PlanRow::None);

    Ok(StateSnapshot {
        spec,
        branch,
        pr,
        drain,
        tests: tests.to_string(),
        fmt: fmt.to_string(),
        plan,
    })
}

/// Classify the branch's push state versus its tracked upstream:
///   * `"pushed"`  — upstream tracks AND local has no commits ahead of it
///   * `"local"`   — no upstream, or local is ahead of upstream
///   * `"unknown"` — the upstream lookup itself failed (git error / detached)
///     trace:TASK-391 | ai:claude
pub(crate) fn describe_push_status(project_root: &std::path::Path, branch: &str) -> String {
    match upstream_ref_for(project_root, branch) {
        None => "local".to_string(),
        Some(u) => match ahead_behind_vs_ref(project_root, branch, &u) {
            Some((0, _behind)) => "pushed".to_string(),
            Some(_) => "local".to_string(),
            None => "unknown".to_string(),
        },
    }
}

/// Build the Drain row from `.aida/drain-state.json` (the live
/// `--auto-complete` orchestrator's record) plus the orchestrator
/// corroboration check. Both inputs are best-effort: missing drain file
/// → "interactive" mode; missing phase env → unknown phase number.
/// trace:TASK-391 | ai:claude
pub(crate) fn describe_drain_row(project_root: &std::path::Path) -> state_snapshot::DrainRow {
    let orchestrator = orchestrator::detect(project_root).is_orchestrated();
    let phase = std::env::var(orchestrator::PHASE_ENV)
        .ok()
        .and_then(|s| s.parse::<u32>().ok());

    let drain_state = drain_state::DrainState::read(project_root);
    let mode = match drain_state {
        None => "interactive".to_string(),
        Some(state) => describe_drain_mode(&state),
    };

    state_snapshot::DrainRow {
        phase,
        mode,
        orchestrator,
    }
}

/// Build the "mode" portion of the Drain row — the human-readable
/// descriptor right of `phase N/6` and left of `orchestrator on|off`:
///   * `"single (<spec>)"`
///   * `"batch <NAME> (<done>/<total> done, <queued> queued)"`
///   * `"next-N (<done>/<total> done, <queued> queued)"`
///     Lives in main.rs (not in `state_snapshot`) so the module stays free of
///     `DrainState` coupling for tests. trace:TASK-391 trace:TASK-417 | ai:claude
pub(crate) fn describe_drain_mode(state: &drain_state::DrainState) -> String {
    let total = state.members.len();
    let done = state
        .members
        .iter()
        .filter(|m| m.state == drain_state::STATE_COMPLETED)
        .count();
    let queued = state
        .members
        .iter()
        .filter(|m| m.state == drain_state::STATE_QUEUED)
        .count();
    match state.mode.as_str() {
        "single" => {
            let spec = state.current.clone().unwrap_or_else(|| "?".to_string());
            format!("single ({spec})")
        }
        "batch" => {
            let name = state.batch.clone().unwrap_or_else(|| "?".to_string());
            format!("batch {name} ({done}/{total} done, {queued} queued)")
        }
        "next-n" => format!("next-{total} ({done}/{total} done, {queued} queued)"),
        other => other.to_string(),
    }
}

/// Find the `docs/plans/` path attached to the active session's manifest,
/// if any — the Plan row's value. Walks `active_lease_for_cwd` to find the
/// session id, then loads `<project_root>/.aida/sessions/<id>.manifest.toml`
/// and reads its `PlanContext`. Best-effort: returns `None` when no
/// session is active, no manifest exists, or no plan was attached.
/// trace:TASK-391 | ai:claude
pub(crate) fn locate_plan_for_active_session(project_root: &std::path::Path) -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let lease = active_lease_for_cwd(project_root, &cwd)?;
    let path = session_manifest::manifest_path(project_root, &lease.id);
    let manifest = session_manifest::load(&path).ok()?;
    manifest.plan.map(|p| p.plan_file)
}
